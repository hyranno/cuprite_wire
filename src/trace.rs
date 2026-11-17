//! Source-code locations used to point errors and warnings back at the circuit description.

use std::{fmt, panic::Location};

/// A location in the circuit description, captured with `#[track_caller]`.
pub type Loc = &'static Location<'static>;

/// A chain of locations, ordered from the outermost subcircuit instantiation
/// to the innermost location (e.g. where a part was instantiated).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Trace(pub Vec<Loc>);

impl Trace {
    pub fn new(loc: Loc) -> Self {
        Trace(vec![loc])
    }

    /// The innermost location.
    pub fn innermost(&self) -> Option<Loc> {
        self.0.last().copied()
    }

    /// Returns this trace as seen from the circuit that instantiated the subcircuit at `loc`.
    pub(crate) fn prepended(&self, loc: Loc) -> Self {
        let mut locations = Vec::with_capacity(self.0.len() + 1);
        locations.push(loc);
        locations.extend_from_slice(&self.0);
        Trace(locations)
    }
}

impl fmt::Display for Trace {
    /// Writes one line per location, innermost first.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, loc) in self.0.iter().rev().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            if i == 0 {
                write!(f, "  at {loc}")?;
            } else {
                write!(f, "  in subcircuit instantiated at {loc}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_innermost_first() {
        let outer = Location::caller();
        let inner = Location::caller();
        let trace = Trace::new(inner).prepended(outer);
        assert_eq!(trace.innermost(), Some(inner));
        assert_eq!(
            trace.to_string(),
            format!("  at {inner}\n  in subcircuit instantiated at {outer}")
        );
    }
}
