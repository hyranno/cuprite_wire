//! The format-independent [`Part`] trait and the electrical types of pins.
//!
//! See the guide [Describing circuits](https://github.com/hyranno/cuprite_wire/blob/main/doc/circuits.md#2-parts).

use std::{
    collections::{BTreeMap, HashMap},
    fmt::{self, Debug},
};

use dyn_clone::DynClone;
use petgraph::graph::NodeIndex;

use crate::{kicad::KicadPartInfo, naming::HierPath, spice::SpicePartInfo, trace::Trace};

/// A part (component) definition.
///
/// The trait itself is format-independent. Format-specific information is returned by
/// [`Part::kicad`] and [`Part::spice`]; `None` means the part cannot be exported to that format.
/// Parts supporting different formats may be mixed in one circuit; exporting a circuit
/// that contains an unsupported part to that format is an error.
pub trait Part: Debug + DynClone {
    /// Reference designator prefix (e.g. `"R"`).
    fn get_ref_base_name(&self) -> String;
    /// IDs of the pins. One pin ID corresponds to one footprint pad.
    fn pin_ids(&self) -> Vec<String>;
    /// The value of the part (e.g. `"10k"`).
    fn value(&self) -> String;
    /// Groups of pins that are connected inside the part (e.g. several GND pins of an IC).
    /// The pins of each group are connected when the part is instantiated.
    fn internal_connections(&self) -> Vec<Vec<String>> {
        vec![]
    }
    fn kicad(&self) -> Option<KicadPartInfo> {
        None
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        None
    }
    /// Electrical types of the pins for ERC, keyed by pin ID. Defaults to those of the KiCad
    /// information. ERC treats pins without a type as [`PinType::Passive`].
    fn pin_types(&self) -> BTreeMap<String, PinType> {
        self.kicad()
            .map(|info| {
                info.pins
                    .into_iter()
                    .map(|(id, pin)| (id, pin.pin_type))
                    .collect()
            })
            .unwrap_or_default()
    }
}
dyn_clone::clone_trait_object!(Part);

#[derive(Debug, Clone)]
pub(crate) struct PartInstance {
    pub part: Box<dyn Part>,
    pub ref_number: u32,
    pub pin_mapping: HashMap<String, NodeIndex>,
    /// Explicit or automatic (`<ref base name>#<n>`) instance name, unique in its subcircuit instance.
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

    pub fn ref_name(&self) -> String {
        format!("{}{}", self.part.get_ref_base_name(), self.ref_number)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PinRef {
    pub node_index: NodeIndex,
    pub part_ref_name: String,
    pub pin_id: String,
}

/// Electrical type of a pin, used by ERC and written as `pintype` in the KiCad netlist.
/// The types and their names are those of KiCad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PinType {
    Input,
    Output,
    Bidirectional,
    TriState,
    #[default]
    Passive,
    Free,
    Unspecified,
    PowerIn,
    PowerOut,
    OpenCollector,
    OpenEmitter,
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
