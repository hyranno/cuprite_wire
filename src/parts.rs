use std::{collections::HashMap, fmt::Debug};

use dyn_clone::DynClone;
use petgraph::graph::NodeIndex;

pub trait Part: Debug + DynClone {
    fn get_ref_base_name(&self) -> String;
}
dyn_clone::clone_trait_object!(Part);

#[derive(Debug, Clone)]
pub(crate) struct PartInstance {
    pub part: Box<dyn Part>,
    pub ref_name: String,
    pub pin_mapping: HashMap<u32, NodeIndex>,
}
