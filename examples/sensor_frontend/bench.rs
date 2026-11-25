//! Test bench shared by the tests of the subcircuits: a supply and SPICE helpers.

use cuprite_wire::{
    circuit::{CircuitBuilder, Named, Node},
    erc::{ErcSettings, check_subcircuit},
    naming::NetLabel,
    parts::PinType,
    spice::{IndependentSource, Simulation, SpiceDeck},
    subcircuit::{IntoSubcircuit, SubcircuitGenerator},
};

use crate::stage::StagePorts;

pub struct Bench {
    pub circuit: CircuitBuilder,
    /// The positive supply, driven by an ideal source.
    pub vcc: Node,
    /// The ground (`GND`, the SPICE ground).
    pub gnd: Node,
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
    pub fn source(&mut self, name: &str, source: IndependentSource) -> Node {
        let pins = self.circuit.instantiate_part(Named(name, source));
        self.circuit.connect(pins[IndependentSource::N], self.gnd);
        pins[IndependentSource::P]
    }

    /// A signal stage on a 5 V supply with an ideal 2.5 V reference, driven by `input`, with its
    /// output probed as `OUT`.
    pub fn stage<S: IntoSubcircuit<(), Interface = StagePorts>>(
        stage: S,
        input: IndependentSource,
    ) -> Self {
        let mut bench = Self::new(IndependentSource::voltage(5.0));
        let ports = bench.circuit.instantiate_subcircuit(stage);
        bench.circuit.connect(ports.vcc, bench.vcc);
        bench.circuit.connect(ports.gnd, bench.gnd);
        let vref = bench.source("vref", IndependentSource::voltage(2.5));
        bench.circuit.connect(ports.vref, vref);
        let input = bench.source("input", input);
        bench.circuit.connect(ports.input, input);
        bench.probe(ports.output, "OUT");
        bench
    }

    /// Names a node, so that the deck can refer to it as `v("/<name>")`.
    pub fn probe(&mut self, node: Node, name: &str) {
        self.circuit.label(node, NetLabel::local(name));
    }

    /// The deck of the bench, to add analyses to.
    pub fn deck(&self) -> SpiceDeck {
        let netlist = self.circuit.build(&()).to_netlist().unwrap();
        netlist.to_spice().unwrap().deck()
    }

    /// Runs ngspice with the front-end `commands`; panics with the log on failure.
    pub fn simulate<S: Into<String>>(&self, commands: impl IntoIterator<Item = S>) -> Simulation {
        run(&self.deck().control(commands))
    }
}

/// Checks a signal stage with ERC, its input and reference driven and its output read.
#[track_caller]
pub fn check_stage<G: SubcircuitGenerator<Options = (), Interface = StagePorts>>(stage: &G) {
    let ports = StagePorts {
        input: Some(PinType::Output),
        vref: Some(PinType::Output),
        vcc: Some(PinType::PowerOut),
        gnd: Some(PinType::PowerOut),
        output: Some(PinType::Input),
    };
    check_subcircuit(stage, &(), &ports, &ErcSettings::default())
        .unwrap()
        .assert_clean();
}

/// Runs `deck`; panics with the log on failure.
pub fn run(deck: &SpiceDeck) -> Simulation {
    deck.run().unwrap_or_else(|error| panic!("{error}\n{deck}"))
}

/// Asserts that `actual` is within `tolerance` of `expected`.
#[track_caller]
pub fn assert_near(name: &str, actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{name} = {actual}, expected {expected} ± {tolerance}"
    );
}
