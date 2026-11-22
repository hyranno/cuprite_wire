//! Errors and warnings reported with the location in the circuit description that caused them.

use std::fmt;

use crate::trace::{Loc, Trace};

/// An additional location related to a diagnostic (e.g. the other one of two duplicates).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Note {
    /// What the location is (e.g. "previously used here").
    pub message: String,
    /// The location the note points to.
    pub trace: Trace,
}

/// An error or a warning with the trace of its cause.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Diagnostic<K> {
    /// What went wrong.
    pub kind: K,
    /// Where in the circuit description the problem was caused.
    pub trace: Trace,
    /// Other related locations.
    pub notes: Vec<Note>,
}

impl<K> Diagnostic<K> {
    /// Creates a diagnostic without notes.
    pub fn new(kind: K, trace: Trace) -> Self {
        Diagnostic {
            kind,
            trace,
            notes: Vec::new(),
        }
    }

    /// Adds a note pointing to another related location.
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

    pub(crate) fn fmt_with_severity(
        &self,
        f: &mut fmt::Formatter<'_>,
        severity: &str,
    ) -> fmt::Result
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

/// The kinds of errors found in a circuit or while exporting it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CircuitErrorKind {
    /// An instance name or a net label is not allowed.
    InvalidName {
        /// The rejected name.
        name: String,
        /// Why the name is not allowed.
        reason: &'static str,
    },
    /// The same explicit instance name is used twice in one (sub)circuit.
    DuplicateInstanceName {
        /// The duplicated instance name.
        name: String,
    },
    /// A part lists the same pin ID more than once.
    DuplicatePinId {
        /// Reference designator of the part.
        ref_name: String,
        /// The duplicated pin ID.
        pin_id: String,
    },
    /// A part refers to a pin ID that it does not have (e.g. in its internal connections).
    UnknownPinId {
        /// Reference designator of the part.
        ref_name: String,
        /// The pin ID that the part does not have.
        pin_id: String,
    },
    /// Two parts get the same reference designator.
    DuplicateRef {
        /// The duplicated reference designator.
        ref_name: String,
    },
    /// Two different nets get the same name.
    DuplicateNetName {
        /// The duplicated net name.
        name: String,
    },
    /// The part has no KiCad information.
    KicadUnsupported {
        /// Reference designator of the part.
        ref_name: String,
    },
    /// The part has no footprint assigned.
    FootprintMissing {
        /// Reference designator of the part.
        ref_name: String,
    },
    /// The KiCad information has no entry for a pin ID of the part.
    PinInfoMissing {
        /// Reference designator of the part.
        ref_name: String,
        /// The pin ID without KiCad pin information.
        pin_id: String,
    },
    /// Two pins of a part are assigned to the same pad.
    DuplicatePad {
        /// Reference designator of the part.
        ref_name: String,
        /// The footprint pad assigned to more than one pin.
        pad: String,
    },
    /// The part has no SPICE information.
    SpiceUnsupported {
        /// Reference designator of the part.
        ref_name: String,
    },
    /// An element of the part cannot be written to a SPICE netlist.
    InvalidSpiceElement {
        /// Reference designator of the part.
        ref_name: String,
        /// What is wrong with the element.
        reason: String,
    },
    /// A pin of the part is not used by its SPICE model and not declared unused.
    SpicePinUnmapped {
        /// Reference designator of the part.
        ref_name: String,
        /// The pin ID not used by the SPICE model.
        pin_id: String,
    },
    /// The name of a net cannot be used as a SPICE node name.
    InvalidSpiceNodeName {
        /// The net name.
        name: String,
        /// Why it cannot be a SPICE node name.
        reason: String,
    },
    /// Two nets get the same SPICE node name (SPICE names are case-insensitive).
    SpiceNodeCollision {
        /// Name of one of the nets.
        name: String,
        /// Name of the other net.
        other: String,
        /// The SPICE node name both nets would get.
        node: String,
    },
    /// Two elements get the same name, ignoring case.
    SpiceElementCollision {
        /// Name of one of the elements.
        name: String,
        /// Name of the other element.
        other: String,
    },
    /// Parts define a SPICE model with the same name differently.
    SpiceModelConflict {
        /// Name of the SPICE model.
        name: String,
    },
    /// No net is the ground of the SPICE netlist.
    GroundMissing,
}

impl fmt::Display for CircuitErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CircuitErrorKind::InvalidName { name, reason } => {
                write!(f, "invalid name {name:?}: {reason}")
            }
            CircuitErrorKind::DuplicateInstanceName { name } => {
                write!(
                    f,
                    "instance name {name:?} is used more than once in the same circuit"
                )
            }
            CircuitErrorKind::DuplicatePinId { ref_name, pin_id } => {
                write!(f, "{ref_name}: pin ID {pin_id:?} is listed more than once")
            }
            CircuitErrorKind::UnknownPinId { ref_name, pin_id } => {
                write!(f, "{ref_name}: unknown pin ID {pin_id:?}")
            }
            CircuitErrorKind::DuplicateRef { ref_name } => {
                write!(f, "reference {ref_name:?} is used by more than one part")
            }
            CircuitErrorKind::DuplicateNetName { name } => {
                write!(f, "net name {name:?} is used by more than one net")
            }
            CircuitErrorKind::KicadUnsupported { ref_name } => {
                write!(f, "{ref_name}: the part cannot be exported to KiCad")
            }
            CircuitErrorKind::FootprintMissing { ref_name } => {
                write!(f, "{ref_name}: no footprint is assigned")
            }
            CircuitErrorKind::PinInfoMissing { ref_name, pin_id } => {
                write!(
                    f,
                    "{ref_name}: no KiCad pin information for pin ID {pin_id:?}"
                )
            }
            CircuitErrorKind::DuplicatePad { ref_name, pad } => {
                write!(
                    f,
                    "{ref_name}: pad {pad:?} is assigned to more than one pin"
                )
            }
            CircuitErrorKind::SpiceUnsupported { ref_name } => {
                write!(f, "{ref_name}: the part cannot be exported to SPICE")
            }
            CircuitErrorKind::InvalidSpiceElement { ref_name, reason } => {
                write!(f, "{ref_name}: {reason}")
            }
            CircuitErrorKind::SpicePinUnmapped { ref_name, pin_id } => write!(
                f,
                "{ref_name}: pin ID {pin_id:?} is not used by the SPICE model (declare it in `unused_pins` if intended)"
            ),
            CircuitErrorKind::InvalidSpiceNodeName { name, reason } => {
                write!(f, "net {name:?} cannot be a SPICE node: {reason}")
            }
            CircuitErrorKind::SpiceNodeCollision { name, other, node } => write!(
                f,
                "nets {name:?} and {other:?} would be merged into SPICE node {node:?} (SPICE names are case-insensitive)"
            ),
            CircuitErrorKind::SpiceElementCollision { name, other } => {
                write!(
                    f,
                    "SPICE element names {name:?} and {other:?} differ only in case"
                )
            }
            CircuitErrorKind::SpiceModelConflict { name } => {
                write!(
                    f,
                    "SPICE model {name:?} is defined differently by more than one part"
                )
            }
            CircuitErrorKind::GroundMissing => {
                write!(f, "no ground net for SPICE: name a net \"GND\" or \"0\"")
            }
        }
    }
}

/// An error in a circuit, with the trace of its cause.
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

impl CircuitErrors {
    /// `Ok(value)` if there is no error.
    pub(crate) fn into_result<T>(errors: Vec<CircuitError>, value: T) -> Result<T, CircuitErrors> {
        if errors.is_empty() {
            Ok(value)
        } else {
            Err(CircuitErrors(errors))
        }
    }
}

impl fmt::Display for CircuitErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_lines(f, &self.0)
    }
}

/// Writes `items` one per line, without a newline after the last one.
pub(crate) fn write_lines(f: &mut fmt::Formatter<'_>, items: &[impl fmt::Display]) -> fmt::Result {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            writeln!(f)?;
        }
        write!(f, "{item}")?;
    }
    Ok(())
}

impl std::error::Error for CircuitErrors {}

/// The kinds of warnings found while converting a circuit to a netlist.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CircuitWarningKind {
    /// A net has several names; `used` is used and the others only connect nets.
    MultipleNetNames {
        /// The name used for the net.
        used: String,
        /// The other names of the net.
        others: Vec<String>,
    },
    /// A labeled net has no part pin, so it does not appear in the netlist.
    LabelOnlyNet {
        /// Names of the net.
        names: Vec<String>,
    },
}

impl fmt::Display for CircuitWarningKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CircuitWarningKind::MultipleNetNames { used, others } => {
                write!(f, "net {used:?} is also named {}", quoted_list(others))
            }
            CircuitWarningKind::LabelOnlyNet { names } => {
                write!(f, "net {} has no part pins", quoted_list(names))
            }
        }
    }
}

fn quoted_list(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("{name:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A warning about a circuit, with the trace of its cause.
pub type CircuitWarning = Diagnostic<CircuitWarningKind>;

impl fmt::Display for CircuitWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_with_severity(f, "warning")
    }
}

/// Reports a warning through the `log` crate. All warnings go through this function.
pub(crate) fn warn(warning: &CircuitWarning) {
    log::warn!("{warning}");
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
