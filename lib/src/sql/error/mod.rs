use nom::error::ErrorKind;
use nom::error::FromExternalError;
use nom::error::ParseError as NomParseError;
use nom::Err;
use std::fmt::Write;
use std::num::ParseFloatError;
use std::num::ParseIntError;
use std::ops::Bound;
use thiserror::Error;

mod utils;
pub use utils::*;
mod render;
pub use render::*;

#[derive(Error, Debug, Clone)]
pub enum ParseError<I> {
	Base(I),
	Expected {
		tried: I,
		expected: &'static str,
	},
	Explained {
		tried: I,
		explained: &'static str,
	},
	ExplainedExpected {
		tried: I,
		explained: &'static str,
		expected: &'static str,
	},
	MissingDelimiter {
		opened: I,
		tried: I,
	},
	ExcessiveDepth(I),
	Field(I, String),
	Split(I, String),
	Order(I, String),
	Group(I, String),
	Role(I, String),
	ParseInt {
		tried: I,
		error: ParseIntError,
	},
	ParseFloat {
		tried: I,
		error: ParseFloatError,
	},
	ParseDecimal {
		tried: I,
		error: rust_decimal::Error,
	},
	ParseRegex {
		tried: I,
		error: regex::Error,
	},
	RangeError {
		tried: I,
		lower: Bound<u32>,
		upper: Bound<u32>,
	},
	InvalidUnicode {
		tried: I,
	},
	InvalidPath {
		tried: I,
		parent: I,
	},
}

impl<I: Clone> ParseError<I> {
	/// returns the input value where the parser failed.
	pub fn tried(&self) -> I {
		let (Self::Base(ref tried)
		| Self::Expected {
			ref tried,
			..
		}
		| Self::Expected {
			ref tried,
			..
		}
		| Self::Explained {
			ref tried,
			..
		}
		| Self::ExplainedExpected {
			ref tried,
			..
		}
		| Self::ExcessiveDepth(ref tried)
		| Self::MissingDelimiter {
			ref tried,
			..
		}
		| Self::Field(ref tried, _)
		| Self::Split(ref tried, _)
		| Self::Order(ref tried, _)
		| Self::Group(ref tried, _)
		| Self::Role(ref tried, _)
		| Self::ParseInt {
			ref tried,
			..
		}
		| Self::ParseFloat {
			ref tried,
			..
		}
		| Self::ParseDecimal {
			ref tried,
			..
		}
		| Self::ParseRegex {
			ref tried,
			..
		}
		| Self::RangeError {
			ref tried,
			..
		}
		| Self::InvalidUnicode {
			ref tried,
			..
		}
		| Self::InvalidPath {
			ref tried,
			..
		}) = self;
		tried.clone()
	}
}

/// A location inside a string.
///
/// Locations are 1 indexed, the first character on the first line being on line 1 column 1.
#[derive(Clone, Copy, Debug)]
pub struct Location {
	pub line: usize,
	pub column: usize,
}

impl Location {
	/// Returns the location of a substring in the larger string.
	pub fn of_in(substr: &str, s: &str) -> Self {
		let offset = s
			.len()
			.checked_sub(substr.len())
			.expect("tried to find location of substring in unrelated string");
		let lines = s.split('\n').enumerate();
		let mut total = 0;
		for (idx, line) in lines {
			// +1 for the '\n'
			let new_total = total + line.len() + 1;
			if new_total > offset {
				// found line.
				let line_offset = offset - total;
				let column = line[..line_offset].chars().count();
				// +1 because line and column are 1 index.
				return Self {
					line: idx + 1,
					column: column + 1,
				};
			}
			total = new_total;
		}
		unreachable!()
	}
}

impl ParseError<&str> {
	/// Returns the error represented as a pretty printed string formatted on the original source
	/// text.
	pub fn render_on(&self, input: &str) -> RenderedError {
		match self {
			ParseError::Base(i) => {
				let location = Location::of_in(i, input);
				let text = format!(
					"Failed to parse query at line {} column {}",
					location.line, location.column
				);
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::Expected {
				tried,
				expected,
			} => {
				let location = Location::of_in(tried, input);
				// Writing to a string can't return an error.
				let text = format!(
					"Failed to parse query at line {} column {} expected {}",
					location.line, location.column, expected
				);
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::Explained {
				tried,
				explained,
			} => {
				let location = Location::of_in(tried, input);
				// Writing to a string can't return an error.
				let text = format!(
					"Failed to parse query at line {} column {}",
					location.line, location.column
				);
				let snippet = Snippet::from_source_location(input, location, Some(*explained));
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::ExplainedExpected {
				tried,
				expected,
				explained,
			} => {
				let location = Location::of_in(tried, input);
				// Writing to a string can't return an error.
				let text = format!(
					"Failed to parse query at line {} column {} expected {}",
					location.line, location.column, expected
				);
				let snippet = Snippet::from_source_location(input, location, Some(*explained));
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::InvalidPath {
				tried,
				parent,
			} => {
				let location = Location::of_in(tried, input);
				// Writing to a string can't return an error.
				let text = format!(
					"Path is not a member of {parent} at line {} column {}",
					location.line, location.column
				);
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::MissingDelimiter {
				tried,
				opened,
			} => {
				let location = Location::of_in(tried, input);
				let text = format!(
					"Missing closing delimiter at line {} column {}",
					location.line, location.column
				);
				let error_snippet = Snippet::from_source_location(input, location, None);
				let location = Location::of_in(opened, input);
				let open_snippet = Snippet::from_source_location(
					input,
					location,
					Some("expected this delimiter to be closed"),
				);
				RenderedError {
					text,
					snippets: vec![error_snippet, open_snippet],
				}
			}
			ParseError::ExcessiveDepth(tried) => {
				let location = Location::of_in(tried, input);
				// Writing to a string can't return an error.
				let text = format!(
					"Exceeded maximum parse depth at line {} column {}",
					location.line, location.column
				);
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::Field(tried, f) => {
				let location = Location::of_in(tried, input);
				let text = format!(
					"Found '{f}' in SELECT clause at line {} column {}, but field is not an aggregate function, and is not present in GROUP BY expression",
					location.line, location.column
				);
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::Split(tried, f) => {
				let location = Location::of_in(tried, input);
				let text = format!(
					"Found '{f}' in SPLIT ON clause at line {} column {}, but field is is not present in SELECT expression",
					location.line, location.column
				);
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::Order(tried, f) => {
				let location = Location::of_in(tried, input);
				let text = format!(
					"Found '{f}' in ORDER BY clause at line {} column {}, but field is is not present in SELECT expression",
					location.line, location.column
				);
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::Group(tried, f) => {
				let location = Location::of_in(tried, input);
				let text = format!(
					"Found '{f}' in GROUP BY clause at line {} column {}, but field is is not present in SELECT expression",
					location.line, location.column
				);
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::Role(tried, r) => {
				let location = Location::of_in(tried, input);
				let text = format!(
					"Invalid role '{r}' at line {} column {}.",
					location.line, location.column
				);
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::ParseInt {
				tried,
				error,
			} => {
				let location = Location::of_in(tried, input);
				// Writing to a string can't return an error.
				let text = format!("Failed to parse '{tried}' as an integer: {error}.");
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::ParseFloat {
				tried,
				error,
			} => {
				let location = Location::of_in(tried, input);
				// Writing to a string can't return an error.
				let text = format!("Failed to parse '{tried}' as a float: {error}.");
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::ParseDecimal {
				tried,
				error,
			} => {
				let location = Location::of_in(tried, input);
				// Writing to a string can't return an error.
				let text = format!("Failed to parse '{tried}' as decimal: {error}.");
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::ParseRegex {
				tried,
				error,
			} => {
				let location = Location::of_in(tried, input);
				// Writing to a string can't return an error.
				let text = format!("Failed to parse '{tried}' as a regex: {error}.");
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}

			ParseError::RangeError {
				tried,
				lower,
				upper,
			} => {
				let location = Location::of_in(tried, input);

				let mut text =
					format!("Failed to parse '{tried}' as a bounded integer with bounds");
				// Writing to a string can't return an error.
				match lower {
					Bound::Included(x) => write!(&mut text, "[{}", x).unwrap(),
					Bound::Excluded(x) => write!(&mut text, "({}", x).unwrap(),
					Bound::Unbounded => {}
				}
				write!(&mut text, "...").unwrap();
				match upper {
					Bound::Included(x) => write!(&mut text, "{}]", x).unwrap(),
					Bound::Excluded(x) => write!(&mut text, "{})", x).unwrap(),
					Bound::Unbounded => {}
				}
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
			ParseError::InvalidUnicode {
				tried,
			} => {
				let location = Location::of_in(tried, input);
				let text = "Invalid unicode escape code.".to_string();
				let snippet = Snippet::from_source_location(input, location, None);
				RenderedError {
					text,
					snippets: vec![snippet],
				}
			}
		}
	}
}

pub type IResult<I, O, E = ParseError<I>> = Result<(I, O), Err<E>>;

impl<I> FromExternalError<I, ParseIntError> for ParseError<I> {
	fn from_external_error(input: I, _kind: ErrorKind, e: ParseIntError) -> Self {
		ParseError::ParseInt {
			error: e,
			tried: input,
		}
	}
}

impl<I> FromExternalError<I, ParseFloatError> for ParseError<I> {
	fn from_external_error(input: I, _kind: ErrorKind, e: ParseFloatError) -> Self {
		ParseError::ParseFloat {
			error: e,
			tried: input,
		}
	}
}

impl<I> FromExternalError<I, regex::Error> for ParseError<I> {
	fn from_external_error(input: I, _kind: ErrorKind, e: regex::Error) -> Self {
		ParseError::ParseRegex {
			error: e,
			tried: input,
		}
	}
}

impl<I> NomParseError<I> for ParseError<I> {
	fn from_error_kind(input: I, _: ErrorKind) -> Self {
		Self::Base(input)
	}
	fn append(_: I, _: ErrorKind, other: Self) -> Self {
		other
	}
}
