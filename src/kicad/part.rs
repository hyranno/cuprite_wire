use std::collections::{BTreeMap, HashSet};

pub use crate::parts::PinType;

pub use cuprite_wire_kicad_lib::naming::{LibId, LibIdError};

/// KiCad information of a pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KicadPinInfo {
    /// Footprint pad number (`"1"`, `"A3"`, ...).
    pub pad: String,
    /// Pin name, written as `pinfunction`.
    pub name: Option<String>,
    pub pin_type: PinType,
}

impl KicadPinInfo {
    pub fn new(pad: impl Into<String>, name: Option<&str>, pin_type: PinType) -> Self {
        KicadPinInfo {
            pad: pad.into(),
            name: name.map(str::to_string),
            pin_type,
        }
    }
}

/// Component flags written as value-less properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PartFlags {
    pub dnp: bool,
    pub exclude_from_bom: bool,
    pub exclude_from_board: bool,
}

/// KiCad-specific information of a part.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KicadPartInfo {
    /// The footprint. Exporting a part without one is an error.
    pub footprint: Option<LibId>,
    /// Pin information keyed by pin ID. Every pin ID of the part needs exactly one entry.
    pub pins: BTreeMap<String, KicadPinInfo>,
    /// The schematic symbol the part corresponds to, if any.
    pub lib_source: Option<LibId>,
    /// Additional fields imported into the footprint.
    pub fields: Vec<(String, String)>,
    pub flags: PartFlags,
}

/// A mismatch between the pin IDs of a part and its KiCad pin information.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PinInfoProblem {
    /// A pin ID of the part has no entry.
    Missing { pin_id: String },
    /// An entry is not a pin ID of the part.
    Unknown { pin_id: String },
    /// Several pins are assigned to the pad.
    DuplicatePad { pad: String },
}

impl KicadPartInfo {
    /// Checks that `pins` has exactly one entry for each of `pin_ids` and that pads are unique.
    pub fn check_pins(&self, pin_ids: &[String]) -> Vec<PinInfoProblem> {
        let mut problems = Vec::new();
        let pin_id_set: HashSet<&String> = pin_ids.iter().collect();
        let mut missing: Vec<_> = pin_id_set
            .iter()
            .filter(|id| !self.pins.contains_key(**id))
            .collect();
        missing.sort();
        problems.extend(missing.into_iter().map(|pin_id| PinInfoProblem::Missing {
            pin_id: pin_id.to_string(),
        }));
        let mut pads = HashSet::new();
        for (pin_id, pin) in &self.pins {
            if !pin_id_set.contains(pin_id) {
                problems.push(PinInfoProblem::Unknown {
                    pin_id: pin_id.clone(),
                });
            }
            if !pads.insert(&pin.pad) {
                problems.push(PinInfoProblem::DuplicatePad {
                    pad: pin.pad.clone(),
                });
            }
        }
        problems
    }

    /// Whether the pin name is used by more than one pin of the part.
    pub(crate) fn is_pin_name_shared(&self, name: &str) -> bool {
        self.pins
            .values()
            .filter(|pin| pin.name.as_deref() == Some(name))
            .count()
            > 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_pins() {
        let info = KicadPartInfo {
            pins: BTreeMap::from([
                ("a".into(), KicadPinInfo::new("1", None, PinType::Passive)),
                ("b".into(), KicadPinInfo::new("1", None, PinType::Passive)),
                ("x".into(), KicadPinInfo::new("2", None, PinType::Passive)),
            ]),
            ..Default::default()
        };
        let pin_ids = ["a", "b", "c"].map(String::from);
        assert_eq!(
            info.check_pins(&pin_ids),
            vec![
                PinInfoProblem::Missing { pin_id: "c".into() },
                PinInfoProblem::DuplicatePad { pad: "1".into() },
                PinInfoProblem::Unknown { pin_id: "x".into() },
            ]
        );
    }
}
