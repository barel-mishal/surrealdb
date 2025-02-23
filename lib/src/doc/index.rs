use crate::ctx::Context;
use crate::dbs::Statement;
use crate::dbs::{Options, Transaction};
use crate::doc::{CursorDoc, Document};
use crate::err::Error;
use crate::idx::ft::FtIndex;
use crate::idx::trees::store::TreeStoreType;
use crate::idx::IndexKeyBase;
use crate::sql::array::Array;
use crate::sql::index::{Index, SearchParams};
use crate::sql::statements::DefineIndexStatement;
use crate::sql::{Part, Thing, Value};
use crate::{key, kvs};

impl<'a> Document<'a> {
	pub async fn index(
		&self,
		ctx: &Context<'_>,
		opt: &Options,
		txn: &Transaction,
		_stm: &Statement<'_>,
	) -> Result<(), Error> {
		// Check indexes
		if !opt.indexes {
			return Ok(());
		}
		// Check if forced
		if !opt.force && !self.changed() {
			return Ok(());
		}
		// Check if the table is a view
		if self.tb(opt, txn).await?.drop {
			return Ok(());
		}
		// Get the record id
		let rid = self.id.as_ref().unwrap();
		// Loop through all index statements
		for ix in self.ix(opt, txn).await?.iter() {
			// Calculate old values
			let o = build_opt_values(ctx, opt, txn, ix, &self.initial).await?;

			// Calculate new values
			let n = build_opt_values(ctx, opt, txn, ix, &self.current).await?;

			// Update the index entries
			if opt.force || o != n {
				// Claim transaction
				let mut run = txn.lock().await;

				// Store all the variable and parameters required by the index operation
				let mut ic = IndexOperation::new(opt, ix, o, n, rid);

				// Index operation dispatching
				match &ix.index {
					Index::Uniq => ic.index_unique(&mut run).await?,
					Index::Idx => ic.index_non_unique(&mut run).await?,
					Index::Search(p) => ic.index_full_text(&mut run, p).await?,
					Index::MTree(_) => {
						return Err(Error::FeatureNotYetImplemented {
							feature: "MTree indexing".to_string(),
						})
					}
				};
			}
		}
		// Carry on
		Ok(())
	}
}

/// Extract from the given document, the values required by the index and put then in an array.
/// Eg. IF the index is composed of the columns `name` and `instrument`
/// Given this doc: { "id": 1, "instrument":"piano", "name":"Tobie" }
/// It will return: ["Tobie", "piano"]
async fn build_opt_values(
	ctx: &Context<'_>,
	opt: &Options,
	txn: &Transaction,
	ix: &DefineIndexStatement,
	doc: &CursorDoc<'_>,
) -> Result<Option<Vec<Value>>, Error> {
	if !doc.doc.is_some() {
		return Ok(None);
	}
	let mut o = Vec::with_capacity(ix.cols.len());
	for i in ix.cols.iter() {
		let v = i.compute(ctx, opt, txn, Some(doc)).await?;
		o.push(v);
	}
	Ok(Some(o))
}

/// Extract from the given document, the values required by the index and put then in an array.
/// Eg. IF the index is composed of the columns `name` and `instrument`
/// Given this doc: { "id": 1, "instrument":"piano", "name":"Tobie" }
/// It will return: ["Tobie", "piano"]
struct Indexable(Vec<(Value, bool)>);

impl Indexable {
	fn new(vals: Vec<Value>, ix: &DefineIndexStatement) -> Self {
		let mut source = Vec::with_capacity(vals.len());
		for (v, i) in vals.into_iter().zip(ix.cols.0.iter()) {
			let f = matches!(i.0.last(), Some(&Part::Flatten));
			source.push((v, f));
		}
		Self(source)
	}
}

impl IntoIterator for Indexable {
	type Item = Array;
	type IntoIter = Combinator;

	fn into_iter(self) -> Self::IntoIter {
		Combinator::new(self.0)
	}
}

struct Combinator {
	iterators: Vec<Box<dyn ValuesIterator>>,
	has_next: bool,
}

impl Combinator {
	fn new(source: Vec<(Value, bool)>) -> Self {
		let mut iterators: Vec<Box<dyn ValuesIterator>> = Vec::new();
		// We create an iterator for each idiom
		for (v, f) in source {
			if !f {
				// Iterator for not flattened values
				if let Value::Array(v) = v {
					iterators.push(Box::new(MultiValuesIterator {
						vals: v.0,
						done: false,
						current: 0,
					}));
					continue;
				}
			}
			iterators.push(Box::new(SingleValueIterator(v)));
		}
		Self {
			iterators,
			has_next: true,
		}
	}
}

impl Iterator for Combinator {
	type Item = Array;

	fn next(&mut self) -> Option<Self::Item> {
		if !self.has_next {
			return None;
		}
		let mut o = Vec::with_capacity(self.iterators.len());
		// Create the combination and advance to the next
		self.has_next = false;
		for i in &mut self.iterators {
			o.push(i.current().clone());
			if !self.has_next {
				// We advance only one iterator per iteration
				if i.next() {
					self.has_next = true;
				}
			}
		}
		let o = Array::from(o);
		Some(o)
	}
}

trait ValuesIterator: Send {
	fn next(&mut self) -> bool;
	fn current(&self) -> &Value;
}

struct MultiValuesIterator {
	vals: Vec<Value>,
	done: bool,
	current: usize,
}

impl ValuesIterator for MultiValuesIterator {
	fn next(&mut self) -> bool {
		if self.done {
			return false;
		}
		if self.current == self.vals.len() - 1 {
			self.done = true;
			return false;
		}
		self.current += 1;
		true
	}

	fn current(&self) -> &Value {
		self.vals.get(self.current).unwrap_or(&Value::Null)
	}
}

struct SingleValueIterator(Value);

impl ValuesIterator for SingleValueIterator {
	fn next(&mut self) -> bool {
		false
	}

	fn current(&self) -> &Value {
		&self.0
	}
}

struct IndexOperation<'a> {
	opt: &'a Options,
	ix: &'a DefineIndexStatement,
	/// The old values (if existing)
	o: Option<Vec<Value>>,
	/// The new values (if existing)
	n: Option<Vec<Value>>,
	rid: &'a Thing,
}

impl<'a> IndexOperation<'a> {
	fn new(
		opt: &'a Options,
		ix: &'a DefineIndexStatement,
		o: Option<Vec<Value>>,
		n: Option<Vec<Value>>,
		rid: &'a Thing,
	) -> Self {
		Self {
			opt,
			ix,
			o,
			n,
			rid,
		}
	}

	fn get_unique_index_key(&self, v: &'a Array) -> key::index::Index {
		crate::key::index::Index::new(
			self.opt.ns(),
			self.opt.db(),
			&self.ix.what,
			&self.ix.name,
			v,
			None,
		)
	}

	fn get_non_unique_index_key(&self, v: &'a Array) -> key::index::Index {
		crate::key::index::Index::new(
			self.opt.ns(),
			self.opt.db(),
			&self.ix.what,
			&self.ix.name,
			v,
			Some(&self.rid.id),
		)
	}

	async fn index_unique(&mut self, run: &mut kvs::Transaction) -> Result<(), Error> {
		// Delete the old index data
		if let Some(o) = self.o.take() {
			let i = Indexable::new(o, self.ix);
			for o in i {
				let key = self.get_unique_index_key(&o);
				match run.delc(key, Some(self.rid)).await {
					Err(Error::TxConditionNotMet) => Ok(()),
					Err(e) => Err(e),
					Ok(v) => Ok(v),
				}?
			}
		}
		// Create the new index data
		if let Some(n) = self.n.take() {
			let i = Indexable::new(n, self.ix);
			for n in i {
				if !n.is_all_none_or_null() {
					let key = self.get_unique_index_key(&n);
					if run.putc(key, self.rid, None).await.is_err() {
						let key = self.get_unique_index_key(&n);
						let val = run.get(key).await?.unwrap();
						let rid: Thing = val.into();
						return self.err_index_exists(rid, n);
					}
				}
			}
		}
		Ok(())
	}

	async fn index_non_unique(&mut self, run: &mut kvs::Transaction) -> Result<(), Error> {
		// Delete the old index data
		if let Some(o) = self.o.take() {
			let i = Indexable::new(o, self.ix);
			for o in i {
				let key = self.get_non_unique_index_key(&o);
				match run.delc(key, Some(self.rid)).await {
					Err(Error::TxConditionNotMet) => Ok(()),
					Err(e) => Err(e),
					Ok(v) => Ok(v),
				}?
			}
		}
		// Create the new index data
		if let Some(n) = self.n.take() {
			let i = Indexable::new(n, self.ix);
			for n in i {
				let key = self.get_non_unique_index_key(&n);
				if run.putc(key, self.rid, None).await.is_err() {
					let key = self.get_non_unique_index_key(&n);
					let val = run.get(key).await?.unwrap();
					let rid: Thing = val.into();
					return self.err_index_exists(rid, n);
				}
			}
		}
		Ok(())
	}

	fn err_index_exists(&self, rid: Thing, n: Array) -> Result<(), Error> {
		Err(Error::IndexExists {
			thing: rid,
			index: self.ix.name.to_string(),
			value: match n.len() {
				1 => n.first().unwrap().to_string(),
				_ => n.to_string(),
			},
		})
	}

	async fn index_full_text(
		&self,
		run: &mut kvs::Transaction,
		p: &SearchParams,
	) -> Result<(), Error> {
		let ikb = IndexKeyBase::new(self.opt, self.ix);
		let az = run.get_db_analyzer(self.opt.ns(), self.opt.db(), p.az.as_str()).await?;
		let mut ft = FtIndex::new(run, az, ikb, p, TreeStoreType::Write).await?;
		if let Some(n) = &self.n {
			ft.index_document(run, self.rid, n).await?;
		} else {
			ft.remove_document(run, self.rid).await?;
		}
		ft.finish(run).await
	}
}
