use std::{collections::HashMap, fmt::Debug};

use dyn_clone::DynClone;
use petgraph::graph::NodeIndex;

use crate::{kicad::KicadPartInfo, spice::SpicePartInfo, trace::Trace};

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
}
dyn_clone::clone_trait_object!(Part);

#[derive(Debug, Clone)]
pub(crate) struct PartInstance {
    pub part: Box<dyn Part>,
    pub ref_number: u32,
    pub pin_mapping: HashMap<String, NodeIndex>,
    /// Where the part was instantiated, through the chain of subcircuit instantiations.
    pub trace: Trace,
}
impl PartInstance {
    pub fn ref_name(&self) -> String {
        format!(
            "{}{}",
            self.part.get_ref_base_name(),
            self.ref_number
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PinRef {
    pub node_index: NodeIndex,
    pub part_ref_name: String,
    pub pin_id: String,
}
