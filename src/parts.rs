use std::{collections::HashMap, fmt::Debug};

use dyn_clone::DynClone;
use petgraph::graph::NodeIndex;

use crate::trace::Trace;

pub trait Part: Debug + DynClone {
    fn get_ref_base_name(&self) -> String;
    fn pin_ids(&self) -> Vec<String>;
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
