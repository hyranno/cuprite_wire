//! Reads KiCad symbol and footprint libraries and SPICE model libraries, and turns a symbol
//! into the description of a cuprite_wire part ([`part::PartDef`]).
//!
//! This crate is used by the `kicad_part!` and `spice_part!` macros of `cuprite_wire` at compile
//! time. It does not depend on `proc_macro`, so it can be tested on its own.
//!
//! It is an implementation detail of `cuprite_wire`, published only because the macros depend on
//! it: its API is not covered by semantic versioning and may change in any release. Use
//! `cuprite_wire` instead; the items it needs from here are re-exported there.

pub mod binding;
pub mod footprint;
pub mod naming;
pub mod part;
pub mod resolve;
pub mod sexpr;
pub mod sim;
pub mod spice_lib;
pub mod symbol;
#[cfg(test)]
mod test_util;
pub mod text_var;
pub mod value;

use std::{collections::BTreeSet, fmt};

/// The first of the items with the same key, in order.
pub(crate) fn unique_by<'a, T: 'a, K: Ord + 'a>(
    items: impl IntoIterator<Item = &'a T>,
    key: impl Fn(&'a T) -> K,
) -> impl Iterator<Item = &'a T> {
    let mut seen = BTreeSet::new();
    items.into_iter().filter(move |item| seen.insert(key(item)))
}

/// An error with a human-readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error {
            message: message.into(),
        }
    }

    /// Prefixes the message with `context: `.
    pub fn context(self, context: impl fmt::Display) -> Self {
        Error::new(format!("{context}: {}", self.message))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}
