use std::{collections::HashMap, fmt::Debug};

use dyn_clone::DynClone;
use petgraph::graph::NodeIndex;

use crate::CircuitBuilder;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubcircuitGenerationOptions {
    // options for subcircuit generation
}
pub trait SubcircuitGenerator: Debug + DynClone {
    fn generate_subcircuit(&self, options: &SubcircuitGenerationOptions) -> CircuitBuilder;
}
dyn_clone::clone_trait_object!(SubcircuitGenerator);

#[derive(Debug, Clone)]
pub struct SubcircuitInstance {
    pub generator: Box<dyn SubcircuitGenerator>,
    pub node_mapping: HashMap<NodeIndex, NodeIndex>,  // HashMap from subcircuit node indices to parent circuit node indices
}
