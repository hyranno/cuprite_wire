//! The format-independent [`Part`] trait and the electrical types of pins.
//!
//! See the guide [Describing circuits](https://github.com/hyranno/cuprite_wire/blob/main/doc/circuits.md#2-parts).

use std::{
    collections::{BTreeMap, HashMap},
    fmt::{self, Debug},
};

use dyn_clone::DynClone;
use petgraph::{graph::NodeIndex, unionfind::UnionFind};

use crate::{kicad::KicadPartInfo, naming::HierPath, spice::SpicePartInfo, trace::Trace};

mod overrides;

pub use overrides::{Overridden, PartExt, PartOverrides};

/// A part (component) definition.
///
/// The trait itself is format-independent. Format-specific information is returned by
/// [`Part::kicad`] and [`Part::spice`]; `None` means the part cannot be exported to that format.
/// Parts supporting different formats may be mixed in one circuit; exporting a circuit
/// that contains an unsupported part to that format is an error.
pub trait Part: Debug + DynClone {
    /// Reference designator prefix (e.g. `"R"`), followed by a number in the reference.
    ///
    /// It must not be empty, end with a digit (`U1` + `1` would be `U11`), or contain whitespace,
    /// `?`, `#`, `/` or `{`; other prefixes are reported as
    /// [`CircuitErrorKind::InvalidName`](crate::error::CircuitErrorKind::InvalidName).
    fn ref_prefix(&self) -> String;
    /// IDs of the pins. One pin ID corresponds to one footprint pad, except for duplicate pins
    /// (several pins on one pad, which KiCad 10 allows): they must be connected by
    /// [`Part::internal_connections`], and the KiCad netlist writes the pad once, with the first
    /// of them in this order.
    fn pin_ids(&self) -> Vec<String>;
    /// The value of the part (e.g. `"10k"`).
    fn value(&self) -> String;
    /// Groups of pins that are connected inside the part (e.g. several GND pins of an IC).
    /// The pins of each group are connected when the part is instantiated.
    fn internal_connections(&self) -> Vec<Vec<String>> {
        vec![]
    }
    /// Pins connected to global nets, as `(pin ID, net name)`: each pin gets the label
    /// [`NetLabel::global`](crate::naming::NetLabel::global) of the name when the part is
    /// instantiated, as KiCad connects a hidden `power_in` pin to the global net named after the
    /// pin. Empty by default.
    fn global_labels(&self) -> Vec<(String, String)> {
        vec![]
    }
    /// The KiCad information (symbol, footprint, pins) of the part, or `None` if it cannot be
    /// exported to a KiCad netlist.
    fn kicad(&self) -> Option<KicadPartInfo> {
        None
    }
    /// The SPICE information of the part, or `None` if it cannot be exported to a SPICE netlist.
    fn spice(&self) -> Option<SpicePartInfo> {
        None
    }
    /// The device of a co-simulation this part is: only
    /// [`CosimDevice`](crate::spice::cosim::CosimDevice) is one.
    #[cfg(feature = "shared-ngspice")]
    #[doc(hidden)]
    fn cosim_device(&self) -> Option<&crate::spice::cosim::CosimDevice> {
        None
    }
    /// Electrical types for ERC of pins that have no KiCad pin information, keyed by pin ID (e.g.
    /// the pins of a part without [`Part::kicad`]). ERC uses the types of the KiCad pin information
    /// and these, and treats pins with neither as [`PinType::Passive`].
    ///
    /// A pin with KiCad pin information is reported as
    /// [`CircuitErrorKind::DuplicatePinType`](crate::error::CircuitErrorKind::DuplicatePinType),
    /// and a key that is not a pin ID as
    /// [`CircuitErrorKind::UnknownPinId`](crate::error::CircuitErrorKind::UnknownPinId). Empty by
    /// default.
    fn extra_pin_types(&self) -> BTreeMap<String, PinType> {
        BTreeMap::new()
    }
}
dyn_clone::clone_trait_object!(Part);

/// The pin structure of a part: its pin IDs and which of them are connected inside the part.
///
/// Overlapping internal connection groups (e.g. the stacked pins `["4", "5"]` and `["4", "ep"]`)
/// are merged into one group, as the pins are connected when the part is instantiated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartPins {
    /// Pin IDs in the order of [`Part::pin_ids`], without duplicates.
    ids: Vec<String>,
    /// Index into `ids` of each pin ID.
    index: HashMap<String, usize>,
    /// The merged internal connection group of each pin, or `None` if it is in none.
    groups: Vec<Option<usize>>,
    /// Pin IDs listed more than once by [`Part::pin_ids`].
    duplicate_ids: Vec<String>,
    /// Pin IDs in the internal connections that are not pin IDs of the part.
    unknown_ids: Vec<String>,
}

impl PartPins {
    /// The pins of `part`, from [`Part::pin_ids`] and [`Part::internal_connections`].
    pub fn of(part: &dyn Part) -> Self {
        PartPins::new(part.pin_ids(), &part.internal_connections())
    }

    /// Pins with the IDs `pin_ids`, connected inside the part by the groups `internal_connections`.
    /// A duplicate pin ID is kept once, and unknown pin IDs in the groups are ignored.
    pub fn new(
        pin_ids: impl IntoIterator<Item = String>,
        internal_connections: &[Vec<String>],
    ) -> Self {
        let mut ids = Vec::new();
        let mut index = HashMap::new();
        let mut duplicate_ids = Vec::new();
        for id in pin_ids {
            if index.contains_key(&id) {
                duplicate_ids.push(id);
            } else {
                index.insert(id.clone(), ids.len());
                ids.push(id);
            }
        }

        let mut union_find = UnionFind::<usize>::new(ids.len());
        let mut grouped = vec![false; ids.len()];
        let mut unknown_ids = Vec::new();
        for group in internal_connections {
            let mut first = None;
            for id in group {
                let Some(&pin) = index.get(id) else {
                    unknown_ids.push(id.clone());
                    continue;
                };
                grouped[pin] = true;
                union_find.union(*first.get_or_insert(pin), pin);
            }
        }
        // Number the groups in the order of their first pins.
        let mut number: HashMap<usize, usize> = HashMap::new();
        let groups = (0..ids.len())
            .map(|pin| {
                grouped[pin].then(|| {
                    let next = number.len();
                    *number.entry(union_find.find(pin)).or_insert(next)
                })
            })
            .collect();
        PartPins {
            ids,
            index,
            groups,
            duplicate_ids,
            unknown_ids,
        }
    }

    /// The pin IDs in the order of [`Part::pin_ids`], without duplicates.
    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// Whether `pin_id` is a pin ID of the part.
    pub fn contains(&self, pin_id: &str) -> bool {
        self.index.contains_key(pin_id)
    }

    /// The internal connection group of `pin_id`, numbered from 0 in the order of the pins, or
    /// `None` if the pin is in no group (or is not a pin of the part).
    pub fn group(&self, pin_id: &str) -> Option<usize> {
        self.groups[*self.index.get(pin_id)?]
    }

    /// Whether `pin_id` is connected inside the part to a pin for which `f` holds.
    pub fn is_connected_to(&self, pin_id: &str, mut f: impl FnMut(&str) -> bool) -> bool {
        let Some(group) = self.group(pin_id) else {
            return false;
        };
        self.ids
            .iter()
            .zip(&self.groups)
            .any(|(other, &other_group)| other_group == Some(group) && f(other))
    }

    /// Pin IDs listed more than once by [`Part::pin_ids`].
    pub(crate) fn duplicate_ids(&self) -> &[String] {
        &self.duplicate_ids
    }

    /// Pin IDs in the internal connections that are not pin IDs of the part.
    pub(crate) fn unknown_ids(&self) -> &[String] {
        &self.unknown_ids
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PartInstance {
    pub part: Box<dyn Part>,
    pub pin_mapping: HashMap<String, NodeIndex>,
    /// Explicit or automatic (`<reference prefix>#<n>`) instance name, unique in its subcircuit instance.
    pub name: String,
    /// Path of the subcircuit instance containing the part.
    pub path: HierPath,
    /// Where the part was instantiated, through the chain of subcircuit instantiations.
    pub trace: Trace,
}
impl PartInstance {
    /// Deterministic UUID derived from the path and the instance name.
    pub fn uuid(&self) -> uuid::Uuid {
        self.path.part_uuid(&self.name)
    }
}

/// Electrical type of a pin, used by ERC and written as `pintype` in the KiCad netlist.
/// The types and their names are those of KiCad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PinType {
    /// `input`: needs a driver on its net.
    Input,
    /// `output`: drives its net.
    Output,
    /// `bidirectional`: an input and output, counted as a driver.
    Bidirectional,
    /// `tri_state`: an output that can be high-impedance, counted as a driver.
    TriState,
    /// `passive`: e.g. a resistor pin, counted as a driver. The default, and the type ERC uses
    /// for pins without one.
    #[default]
    Passive,
    /// `free`: never reported as not connected, and by default conflicts only with `no_connect`.
    Free,
    /// `unspecified`: by default a warning when connected to anything but a `free` pin.
    Unspecified,
    /// `power_in`: needs a `power_out` pin on its net.
    PowerIn,
    /// `power_out`: drives `power_in` pins (and inputs).
    PowerOut,
    /// `open_collector`: an output that only sinks current.
    OpenCollector,
    /// `open_emitter`: an output that only sources current.
    OpenEmitter,
    /// `no_connect`: must be left unconnected; by default an error when connected to any pin.
    NoConnect,
}

impl PinType {
    /// All types, in the order of KiCad's pin conflict matrix (`erc.pin_map` in `.kicad_pro`).
    pub const ALL: [PinType; 12] = [
        PinType::Input,
        PinType::Output,
        PinType::Bidirectional,
        PinType::TriState,
        PinType::Passive,
        PinType::Free,
        PinType::Unspecified,
        PinType::PowerIn,
        PinType::PowerOut,
        PinType::OpenCollector,
        PinType::OpenEmitter,
        PinType::NoConnect,
    ];

    /// The index of the type in [`PinType::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// The KiCad name of the type (e.g. `"power_in"`), as written in the netlist and in ERC messages.
    pub fn as_str(self) -> &'static str {
        match self {
            PinType::Input => "input",
            PinType::Output => "output",
            PinType::Bidirectional => "bidirectional",
            PinType::TriState => "tri_state",
            PinType::Passive => "passive",
            PinType::Free => "free",
            PinType::Unspecified => "unspecified",
            PinType::PowerIn => "power_in",
            PinType::PowerOut => "power_out",
            PinType::OpenCollector => "open_collector",
            PinType::OpenEmitter => "open_emitter",
            PinType::NoConnect => "no_connect",
        }
    }
}

impl fmt::Display for PinType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_parts::strings;

    #[test]
    fn test_part_pins() {
        let pins = PartPins::new(
            strings(&["1", "2", "3", "4", "5", "ep", "2"]),
            &[
                strings(&["4", "5"]),
                strings(&["2", "x"]),
                strings(&["ep", "4"]),
            ],
        );
        assert_eq!(pins.ids(), strings(&["1", "2", "3", "4", "5", "ep"]));
        assert_eq!(pins.duplicate_ids(), strings(&["2"]));
        assert_eq!(pins.unknown_ids(), strings(&["x"]));
        // Overlapping groups are merged, and groups are numbered in the order of the pins.
        let groups: Vec<_> = pins.ids().iter().map(|id| pins.group(id)).collect();
        assert_eq!(groups, [None, Some(0), None, Some(1), Some(1), Some(1)]);
        assert_eq!(pins.group("x"), None);
        assert!(pins.contains("ep") && !pins.contains("x"));
        assert!(pins.is_connected_to("5", |id| id == "ep"));
        assert!(!pins.is_connected_to("1", |_| true));
        assert!(!pins.is_connected_to("2", |id| id == "3"));
    }
}
