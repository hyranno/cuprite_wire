//! KiCad-specific information of parts: footprint, symbol, pins and fields.

use std::collections::BTreeMap;

use crate::parts::{PartPins, PinType};

pub use cuprite_wire_kicad_lib::naming::{LibId, LibIdError, unit_letters};

/// KiCad information of a pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KicadPinInfo {
    /// Footprint pad number (`"1"`, `"A3"`, ...).
    pub pad: String,
    /// Pin name, written as `pinfunction`.
    pub name: Option<String>,
    /// Electrical type, written as `pintype` and used by ERC.
    pub pin_type: PinType,
    /// The symbol unit the pin belongs to, from 1 ([`KicadPartInfo::units`]); 0 for a pin of every
    /// unit (or of a part with a single unit). A pin repeated in several units is given the lowest.
    pub unit: u32,
}

impl KicadPinInfo {
    /// Pin information with the pad `pad`, the name `name` and the type `pin_type`, in every unit.
    pub fn new(pad: impl Into<String>, name: Option<&str>, pin_type: PinType) -> Self {
        KicadPinInfo {
            pad: pad.into(),
            name: name.map(str::to_string),
            pin_type,
            unit: 0,
        }
    }

    /// The pin in the unit `unit` (from 1; 0 for every unit).
    #[must_use]
    pub fn with_unit(mut self, unit: u32) -> Self {
        self.unit = unit;
        self
    }
}

/// Component flags written as value-less properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PartFlags {
    /// Do not populate (`dnp`).
    pub dnp: bool,
    /// Excluded from the bill of materials (`exclude_from_bom`).
    pub exclude_from_bom: bool,
    /// Not placed on the board (`exclude_from_board`).
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
    /// The flags of the component.
    pub flags: PartFlags,
    /// The datasheet (usually a URL), written as `datasheet` and shown in the BOM.
    pub datasheet: Option<String>,
    /// The description, written as `description` and shown in the BOM.
    pub description: Option<String>,
    /// For a part made from a symbol with several units, what KiCad appends to the reference
    /// for each unit, from unit 1: the letters `A`, `B`, ... ([`unit_letters`]), or the unit's
    /// name if the symbol gives one. Empty (or with one entry) for a single unit.
    ///
    /// The automatic net names use the reference of the pin's unit (`Net-(U1B-OUT)`), and the
    /// netlist writes a UUID for each unit in `tstamps`, as KiCad does.
    pub units: Vec<String>,
}

/// A mismatch between the pin IDs of a part and its KiCad pin information.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PinInfoProblem {
    /// A pin ID of the part has no entry.
    Missing {
        /// The pin ID without an entry.
        pin_id: String,
    },
    /// An entry is not a pin ID of the part.
    Unknown {
        /// The key of the entry.
        pin_id: String,
    },
    /// Several pins are assigned to the pad, and they are not connected inside the part
    /// (duplicate pins, which KiCad 10 allows, are).
    DuplicatePad {
        /// The pad number.
        pad: String,
    },
    /// The unit of an entry is not one of [`KicadPartInfo::units`].
    UnknownUnit {
        /// The key of the entry.
        pin_id: String,
        /// The unit of the entry.
        unit: u32,
    },
}

impl KicadPartInfo {
    /// Checks that `pins` has exactly one entry for each pin of `part_pins`, that pins sharing a
    /// pad are connected inside the part, and that the units of the pins are in `units`.
    pub fn check_pins(&self, part_pins: &PartPins) -> Vec<PinInfoProblem> {
        let mut missing: Vec<_> = part_pins
            .ids()
            .iter()
            .filter(|id| !self.pins.contains_key(*id))
            .collect();
        missing.sort();
        let mut problems: Vec<_> = missing
            .into_iter()
            .map(|pin_id| PinInfoProblem::Missing {
                pin_id: pin_id.clone(),
            })
            .collect();
        // Pad -> internal connection groups of its pins (`None` for a pin in no group). Unknown
        // pins are reported as such and not on their pads.
        let mut pads: BTreeMap<&str, Vec<Option<usize>>> = BTreeMap::new();
        for (pin_id, pin) in &self.pins {
            if part_pins.contains(pin_id) {
                pads.entry(&pin.pad)
                    .or_default()
                    .push(part_pins.group(pin_id));
            } else {
                problems.push(PinInfoProblem::Unknown {
                    pin_id: pin_id.clone(),
                });
            }
        }
        // Several pins on a pad are duplicate pins if they are all connected inside the part.
        problems.extend(
            pads.into_iter()
                .filter(|(_, groups)| {
                    groups.len() > 1
                        && (groups[0].is_none() || groups.iter().any(|g| *g != groups[0]))
                })
                .map(|(pad, _)| PinInfoProblem::DuplicatePad { pad: pad.into() }),
        );
        problems.extend(
            self.pins
                .iter()
                .filter(|(_, pin)| pin.unit > 0 && pin.unit > self.unit_count())
                .map(|(pin_id, pin)| PinInfoProblem::UnknownUnit {
                    pin_id: pin_id.clone(),
                    unit: pin.unit,
                }),
        );
        problems
    }

    /// The number of units (1 for a single unit).
    pub fn unit_count(&self) -> u32 {
        u32::try_from(self.units.len()).unwrap_or(u32::MAX).max(1)
    }

    /// Whether `pin`, as a pin of the unit `unit` (0 for a single unit), shares its name with
    /// another pin of that unit, as KiCad decides whether an automatic net name needs the pad: the
    /// other pin is on another pad, and is not a no-connect pin.
    pub(crate) fn is_pin_name_shared(&self, pin: &KicadPinInfo, unit: u32) -> bool {
        self.pins.values().any(|other| {
            other.name == pin.name
                && other.pad != pin.pad
                && other.pin_type != PinType::NoConnect
                && (unit == 0 || other.unit == 0 || other.unit == unit)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_parts::strings;

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
        let part_pins = PartPins::new(["a", "b", "c"].map(String::from), &[]);
        assert_eq!(
            info.check_pins(&part_pins),
            vec![
                PinInfoProblem::Missing { pin_id: "c".into() },
                PinInfoProblem::Unknown { pin_id: "x".into() },
                PinInfoProblem::DuplicatePad { pad: "1".into() },
            ]
        );
        // A pad is reported once, however many pins share it; unknown pins are not on pads.
        let crowded = KicadPartInfo {
            pins: BTreeMap::from([
                ("a".into(), KicadPinInfo::new("1", None, PinType::Passive)),
                ("b".into(), KicadPinInfo::new("1", None, PinType::Passive)),
                ("c".into(), KicadPinInfo::new("1", None, PinType::Passive)),
                ("x".into(), KicadPinInfo::new("2", None, PinType::Passive)),
                ("y".into(), KicadPinInfo::new("2", None, PinType::Passive)),
            ]),
            ..Default::default()
        };
        let crowded_pins = PartPins::new(["a", "b", "c"].map(String::from), &[]);
        assert_eq!(
            crowded.check_pins(&crowded_pins),
            vec![
                PinInfoProblem::Unknown { pin_id: "x".into() },
                PinInfoProblem::Unknown { pin_id: "y".into() },
                PinInfoProblem::DuplicatePad { pad: "1".into() },
            ]
        );
        // Pins on one pad may be connected inside the part (duplicate pins).
        let part_pins = PartPins::new(["a", "b", "x"].map(String::from), &[strings(&["a", "b"])]);
        assert_eq!(info.check_pins(&part_pins), vec![]);
        let part_pins = PartPins::new(["a", "b", "x"].map(String::from), &[strings(&["b", "x"])]);
        assert_eq!(
            info.check_pins(&part_pins),
            vec![PinInfoProblem::DuplicatePad { pad: "1".into() }]
        );
    }

    #[test]
    fn test_check_pin_units() {
        let pin = |pad: &str, unit| KicadPinInfo::new(pad, None, PinType::Passive).with_unit(unit);
        let mut info = KicadPartInfo {
            pins: BTreeMap::from([
                ("a".into(), pin("1", 0)),
                ("b".into(), pin("2", 1)),
                ("c".into(), pin("3", 2)),
            ]),
            ..Default::default()
        };
        let part_pins = PartPins::new(["a", "b", "c"].map(String::from), &[]);
        // A single unit takes the pins of unit 1 too.
        assert_eq!(info.unit_count(), 1);
        assert_eq!(
            info.check_pins(&part_pins),
            vec![PinInfoProblem::UnknownUnit {
                pin_id: "c".into(),
                unit: 2
            }]
        );
        info.units = strings(&["A", "B"]);
        assert_eq!(info.unit_count(), 2);
        assert_eq!(info.check_pins(&part_pins), vec![]);
    }

    #[test]
    fn test_is_pin_name_shared() {
        let pin = |pad: &str, name, pin_type, unit| {
            KicadPinInfo::new(pad, Some(name), pin_type).with_unit(unit)
        };
        let info = KicadPartInfo {
            pins: BTreeMap::from([
                ("1".into(), pin("1", "K", PinType::Passive, 1)),
                ("2".into(), pin("2", "K", PinType::Passive, 2)),
                ("3".into(), pin("3", "GND", PinType::PowerIn, 0)),
                ("4".into(), pin("4", "GND", PinType::PowerIn, 2)),
                // A duplicate pin on pad 1 and a no-connect pin do not count.
                ("1#2".into(), pin("1", "K", PinType::Passive, 1)),
                ("5".into(), pin("5", "K", PinType::NoConnect, 1)),
            ]),
            units: strings(&["A", "B"]),
            ..Default::default()
        };
        let shared = |id: &str, unit| info.is_pin_name_shared(&info.pins[id], unit);
        assert!(!shared("1", 1));
        assert!(shared("1", 0));
        assert!(!shared("3", 1));
        assert!(shared("3", 2));
        assert!(shared("4", 2));
    }
}
