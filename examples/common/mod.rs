//! Parts and subcircuits shared by the examples.

use cuprite_wire::{
    basic::{Capacitor0603, Resistor0603},
    circuit::{CircuitBuilder, Named, NodeIndex},
    naming::NetLabel,
    subcircuit::SubcircuitGenerator,
};

/// One RC stage. Interface nodes: 0 = input, 1 = output.
#[derive(Debug, Clone)]
pub struct RcStage;

impl SubcircuitGenerator for RcStage {
    type Options = ();
    type Interface = [NodeIndex; 2];

    fn construct_subcircuit(
        &self,
        _options: &(),
        circuit: &mut CircuitBuilder<()>,
        interface: Self::Interface,
    ) {
        let [input, output] = interface;
        let r = circuit.instantiate_part(Named("r", Resistor0603::new("10k")));
        let c = circuit.instantiate_part(Named("c", Capacitor0603::new("100n")));
        circuit.connect(input, r["1"]);
        circuit.connect(r["2"], output);
        circuit.connect(output, c["1"]);
        circuit.label(output, NetLabel::local("OUT"));
        circuit.label(c["2"], NetLabel::global("GND"));
    }
}

/// Two [`RcStage`]s in series, with the input labeled `IN`.
/// Returns the circuit and its input and output nodes.
pub fn two_stage_filter() -> (CircuitBuilder, NodeIndex, NodeIndex) {
    let mut circuit = CircuitBuilder::new();
    let first = circuit.instantiate_subcircuit(Named("stage1", RcStage));
    let second = circuit.instantiate_subcircuit(Named("stage2", RcStage));
    circuit.connect(first[1], second[0]);
    let input = first[0];
    circuit.label(input, NetLabel::local("IN"));
    (circuit, input, second[1])
}
