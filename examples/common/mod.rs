//! Parts and subcircuits shared by the examples.

use std::collections::HashSet;

use cuprite_wire::{
    basic::{Capacitor0603, Resistor0603},
    circuit::CircuitBuilder,
    naming::NetLabel,
    subcircuit::SubcircuitGenerator,
};
use petgraph::graph::NodeIndex;

/// One RC stage. Interface nodes: 0 = input, 1 = output.
#[derive(Debug, Clone)]
pub struct RcStage;

impl SubcircuitGenerator for RcStage {
    type Options = ();

    fn construct_subcircuit(&self, _options: &()) -> CircuitBuilder<()> {
        let mut circuit = CircuitBuilder::new();
        let input = circuit.add_node();
        let output = circuit.add_node();
        let r = circuit.instantiate_part_named("r", Box::new(Resistor0603::new("10k")));
        let c = circuit.instantiate_part_named("c", Box::new(Capacitor0603::new("100n")));
        circuit.connect(input, r["1"]);
        circuit.connect(r["2"], output);
        circuit.connect(output, c["1"]);
        circuit.label(output, NetLabel::local("OUT"));
        circuit.label(c["2"], NetLabel::global("GND"));
        circuit
    }
}

/// Two [`RcStage`]s in series, with the input labeled `IN`.
/// Returns the circuit and its input and output nodes.
pub fn two_stage_filter() -> (CircuitBuilder, NodeIndex, NodeIndex) {
    let interface = HashSet::from([NodeIndex::new(0), NodeIndex::new(1)]);
    let mut circuit = CircuitBuilder::new();
    let first = circuit.instantiate_subcircuit_named("stage1", RcStage, &interface, |o: &()| *o);
    let second = circuit.instantiate_subcircuit_named("stage2", RcStage, &interface, |o: &()| *o);
    circuit.connect(first[&NodeIndex::new(1)], second[&NodeIndex::new(0)]);
    let input = first[&NodeIndex::new(0)];
    circuit.label(input, NetLabel::local("IN"));
    (circuit, input, second[&NodeIndex::new(1)])
}
