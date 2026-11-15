//! Errors and warnings reported with the location in the circuit description that caused them.

use std::fmt;

use crate::trace::{Loc, Trace};

/// An additional location related to a diagnostic (e.g. the other one of two duplicates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub message: String,
    pub trace: Trace,
}

/// An error or a warning with the trace of its cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic<K> {
    pub kind: K,
    pub trace: Trace,
    pub notes: Vec<Note>,
}

impl<K> Diagnostic<K> {
    pub fn new(kind: K, trace: Trace) -> Self {
        Diagnostic {
            kind,
            trace,
            notes: Vec::new(),
        }
    }

    pub fn with_note(mut self, message: impl Into<String>, trace: Trace) -> Self {
        self.notes.push(Note {
            message: message.into(),
            trace,
        });
        self
    }

    /// Returns this diagnostic as seen from the circuit that instantiated the subcircuit at `loc`.
    pub(crate) fn prepended(mut self, loc: Loc) -> Self {
        self.trace = self.trace.prepended(loc);
        for note in &mut self.notes {
            note.trace = note.trace.prepended(loc);
        }
        self
    }

    fn fmt_with_severity(&self, f: &mut fmt::Formatter<'_>, severity: &str) -> fmt::Result
    where
        K: fmt::Display,
    {
        write!(f, "{severity}: {}", self.kind)?;
        if !self.trace.0.is_empty() {
            write!(f, "\n{}", self.trace)?;
        }
        for note in &self.notes {
            write!(f, "\nnote: {}", note.message)?;
            if !note.trace.0.is_empty() {
                write!(f, "\n{}", note.trace)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CircuitErrorKind {
    /// An instance name or a net label is not allowed.
    InvalidName { name: String, reason: &'static str },
    /// The same explicit instance name is used twice in one (sub)circuit.
    DuplicateInstanceName { name: String },
}

impl fmt::Display for CircuitErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CircuitErrorKind::InvalidName { name, reason } => {
                write!(f, "invalid name {name:?}: {reason}")
            }
            CircuitErrorKind::DuplicateInstanceName { name } => {
                write!(f, "instance name {name:?} is used more than once in the same circuit")
            }
        }
    }
}

pub type CircuitError = Diagnostic<CircuitErrorKind>;

impl fmt::Display for CircuitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_with_severity(f, "error")
    }
}

impl std::error::Error for CircuitError {}

/// All errors found in a circuit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CircuitErrors(pub Vec<CircuitError>);

impl fmt::Display for CircuitErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, error) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CircuitErrors {}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CircuitWarningKind {}

impl fmt::Display for CircuitWarningKind {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

pub type CircuitWarning = Diagnostic<CircuitWarningKind>;

impl fmt::Display for CircuitWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_with_severity(f, "warning")
    }
}

/// Reports a warning. All warnings go through this function.
#[allow(dead_code)]
pub(crate) fn warn(warning: &CircuitWarning) {
    eprintln!("{warning}");
}

#[cfg(test)]
mod tests {
    use std::panic::Location;

    use super::*;

    #[test]
    fn test_display_error_with_note() {
        let inner = Location::caller();
        let other = Location::caller();
        let outer = Location::caller();
        let error = CircuitError::new(
            CircuitErrorKind::DuplicateInstanceName { name: "a".into() },
            Trace::new(inner),
        )
        .with_note("previously used here", Trace::new(other))
        .prepended(outer);
        assert_eq!(
            error.to_string(),
            format!(
                "error: instance name \"a\" is used more than once in the same circuit\n  at {inner}\n  in subcircuit instantiated at {outer}\nnote: previously used here\n  at {other}\n  in subcircuit instantiated at {outer}"
            )
        );
    }
}
