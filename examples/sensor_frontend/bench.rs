//! Test bench shared by the tests of the subcircuits: a supply and SPICE helpers.

use cuprite_wire::{
    circuit::{CircuitBuilder, Named, NodeIndex},
    naming::NetLabel,
    spice::{IndependentSource, Simulation},
};

/// The nodes of a signal stage (the preamp, the filter) for [`Bench::stage`].
pub struct StageNodes {
    pub input: NodeIndex,
    pub vref: NodeIndex,
    pub vcc: NodeIndex,
    pub gnd: NodeIndex,
    pub output: NodeIndex,
}

pub struct Bench {
    pub circuit: CircuitBuilder,
    /// The positive supply, driven by an ideal source.
    pub vcc: NodeIndex,
    /// The ground (`GND`, the SPICE ground).
    pub gnd: NodeIndex,
}

impl Bench {
    /// A bench powered by `supply`, e.g. `IndependentSource::voltage(5.0)`.
    pub fn new(supply: IndependentSource) -> Self {
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
    pub fn source(&mut self, name: &str, source: IndependentSource) -> NodeIndex {
        let pins = self.circuit.instantiate_part(Named(name, source));
        self.circuit.connect(pins[IndependentSource::N], self.gnd);
        pins[IndependentSource::P]
    }

    /// A signal stage on a 5 V supply with an ideal 2.5 V reference, driven by `input`, with its
    /// output probed as `OUT`. `instantiate` adds the stage and returns its nodes.
    pub fn stage(
        input: IndependentSource,
        instantiate: impl FnOnce(&mut CircuitBuilder) -> StageNodes,
    ) -> Self {
        let mut bench = Self::new(IndependentSource::voltage(5.0));
        let nodes = instantiate(&mut bench.circuit);
        bench.circuit.connect(nodes.vcc, bench.vcc);
        bench.circuit.connect(nodes.gnd, bench.gnd);
        let vref = bench.source("vref", IndependentSource::voltage(2.5));
        bench.circuit.connect(nodes.vref, vref);
        let input = bench.source("input", input);
        bench.circuit.connect(nodes.input, input);
        bench.probe(nodes.output, "OUT");
        bench
    }

    /// Names a node, so that the deck can refer to it as `v("/<name>")`.
    pub fn probe(&mut self, node: NodeIndex, name: &str) {
        self.circuit.label(node, NetLabel::local(name));
    }

    /// Runs ngspice with the front-end `commands`; panics with the log on failure.
    pub fn simulate<S: Into<String>>(&self, commands: impl IntoIterator<Item = S>) -> Simulation {
        let netlist = self
            .circuit
            .build(&())
            .to_netlist()
            .unwrap()
            .to_spice()
            .unwrap();
        let deck = netlist.deck().control(commands);
        deck.run().unwrap_or_else(|error| panic!("{error}\n{deck}"))
    }
}

/// Asserts that `actual` is within `tolerance` of `expected`.
#[track_caller]
pub fn assert_near(name: &str, actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{name} = {actual}, expected {expected} ± {tolerance}"
    );
}
