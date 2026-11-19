//! Test bench shared by the tests of the subcircuits: a supply and SPICE helpers.

use cuprite_wire::{
    circuit::CircuitBuilder,
    naming::NetLabel,
    spice::{IndependentSource, Simulation},
};
use petgraph::graph::NodeIndex;

pub struct Bench {
    pub circuit: CircuitBuilder,
    /// The positive supply, driven by an ideal source.
    pub vcc: NodeIndex,
    /// The ground (`GND`, the SPICE ground).
    pub gnd: NodeIndex,
}

impl Bench {
    /// A bench powered by `supply`, e.g. `IndependentSource::voltage(5.0)`.
    pub fn new(supply: Box<IndependentSource>) -> Self {
        let mut circuit = CircuitBuilder::new();
        let vcc = circuit.add_node();
        let gnd = circuit.add_node();
        circuit.label(vcc, NetLabel::local("VCC"));
        circuit.label(gnd, NetLabel::global("GND"));
        let mut bench = Self { circuit, vcc, gnd };
        let supply_node = bench.source("supply", supply);
        bench.circuit.connect(supply_node, vcc);
        bench
    }

    /// Adds a source between a new node (returned) and the ground.
    pub fn source(&mut self, name: &str, source: Box<IndependentSource>) -> NodeIndex {
        let pins = self.circuit.instantiate_part_named(name, source);
        self.circuit.connect(pins["n"], self.gnd);
        pins["p"]
    }

    /// Names a node, so that the deck can refer to it as `v("/<name>")`.
    pub fn probe(&mut self, node: NodeIndex, name: &str) {
        self.circuit.label(node, NetLabel::local(name));
    }

    /// Runs ngspice with the front-end `commands`; panics with the log on failure.
    pub fn simulate<S: Into<String>>(&self, commands: impl IntoIterator<Item = S>) -> Simulation {
        let netlist = self.circuit.build(&()).to_netlist().unwrap().to_spice().unwrap();
        let deck = netlist.deck().control(commands);
        deck.run().unwrap_or_else(|error| panic!("{error}\n{deck}"))
    }
}

/// A value printed or measured by the simulation, e.g. by `meas`.
pub fn value(simulation: &Simulation, name: &str) -> f64 {
    simulation
        .value(name)
        .unwrap_or_else(|| panic!("{name} not found:\n{}", simulation.log()))
}

/// Asserts that `actual` is within `tolerance` of `expected`.
#[track_caller]
pub fn assert_near(name: &str, actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{name} = {actual}, expected {expected} ± {tolerance}"
    );
}
