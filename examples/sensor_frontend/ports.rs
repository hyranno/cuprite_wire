//! Interface nodes of the subcircuits.
//!
//! A subcircuit creates its interface nodes first, so they are the nodes `0..N` of the
//! subcircuit; the same order is used to instantiate it and to check it alone.

use std::collections::HashSet;

use cuprite_wire::{circuit::CircuitBuilder, subcircuit::SubcircuitGenerator};
use petgraph::graph::NodeIndex;

/// The interface nodes `0..N` of a subcircuit, as seen from inside it.
pub fn interface<const N: usize>() -> [NodeIndex; N] {
    std::array::from_fn(NodeIndex::new)
}

/// Creates the interface nodes inside a subcircuit; call it before adding any other node.
pub fn add_interface<const N: usize>(circuit: &mut CircuitBuilder) -> [NodeIndex; N] {
    let nodes = std::array::from_fn(|_| circuit.add_node());
    debug_assert_eq!(
        nodes,
        interface::<N>(),
        "interface nodes must be created first"
    );
    nodes
}

/// Instantiates a subcircuit and returns the nodes of its `N` interface nodes in this circuit.
#[track_caller]
pub fn instantiate<G, const N: usize>(
    circuit: &mut CircuitBuilder,
    name: &str,
    generator: G,
) -> [NodeIndex; N]
where
    G: SubcircuitGenerator<Options = ()> + 'static,
{
    let nodes = interface::<N>();
    let mapping =
        circuit.instantiate_subcircuit_named(name, generator, &HashSet::from(nodes), |o: &()| *o);
    nodes.map(|node| mapping[&node])
}
