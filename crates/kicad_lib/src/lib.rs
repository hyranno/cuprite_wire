//! Reads KiCad symbol and footprint libraries and SPICE model libraries, and turns a symbol
//! into the description of a cuprite_wire part ([`part::PartDef`]).
//!
//! This crate is used by the `kicad_part!` macro at compile time. It does not depend on
//! `proc_macro`, so it can be tested and used on its own.

pub mod binding;
pub mod footprint;
pub mod part;
pub mod resolve;
pub mod sexpr;
pub mod sim;
pub mod spice_lib;
pub mod symbol;
pub mod value;

use std::fmt;

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
