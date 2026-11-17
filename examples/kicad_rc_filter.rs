//! Writes the KiCad netlist of a two-stage RC low-pass filter to stdout.
//!
//! Run with `cargo run --example kicad_rc_filter > rc_filter.net`.

use std::{
    collections::{BTreeMap, HashSet},
    io,
};

use cuprite_wire::{
    circuit::CircuitBuilder,
    kicad::{KicadPartInfo, KicadPinInfo, PinType},
    naming::NetLabel,
    parts::Part,
    subcircuit::SubcircuitGenerator,
};
use petgraph::graph::NodeIndex;

/// A two-terminal passive part (resistor or capacitor) in an 0603 package.
#[derive(Debug, Clone)]
struct Passive {
    ref_base_name: &'static str,
    symbol: &'static str,
    footprint: &'static str,
    value: String,
}

impl Passive {
    fn resistor(value: &str) -> Box<Self> {
        Box::new(Passive {
            ref_base_name: "R",
            symbol: "Device:R",
            footprint: "Resistor_SMD:R_0603_1608Metric",
            value: value.into(),
        })
    }

    fn capacitor(value: &str) -> Box<Self> {
        Box::new(Passive {
            ref_base_name: "C",
            symbol: "Device:C",
            footprint: "Capacitor_SMD:C_0603_1608Metric",
            value: value.into(),
        })
    }
}

impl Part for Passive {
    fn get_ref_base_name(&self) -> String {
        self.ref_base_name.into()
    }

    fn pin_ids(&self) -> Vec<String> {
        vec!["1".into(), "2".into()]
    }

    fn value(&self) -> String {
        self.value.clone()
    }

    fn kicad(&self) -> Option<KicadPartInfo> {
        Some(KicadPartInfo {
            footprint: Some(self.footprint.parse().unwrap()),
            pins: BTreeMap::from([
                ("1".into(), KicadPinInfo::new("1", None, PinType::Passive)),
                ("2".into(), KicadPinInfo::new("2", None, PinType::Passive)),
            ]),
            lib_source: Some(self.symbol.parse().unwrap()),
            ..Default::default()
        })
    }
}

/// One RC stage. Interface nodes: 0 = input, 1 = output.
#[derive(Debug, Clone)]
struct RcStage;

impl SubcircuitGenerator for RcStage {
    type Options = ();

    fn construct_subcircuit(&self, _options: &()) -> CircuitBuilder<()> {
        let mut circuit = CircuitBuilder::new();
        let input = circuit.add_node();
        let output = circuit.add_node();
        let r = circuit.instantiate_part_named("r", Passive::resistor("10k"));
        let c = circuit.instantiate_part_named("c", Passive::capacitor("100n"));
        circuit.connect(input, r["1"]);
        circuit.connect(r["2"], output);
        circuit.connect(output, c["1"]);
        circuit.label(output, NetLabel::local("OUT"));
        circuit.label(c["2"], NetLabel::global("GND"));
        circuit
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let interface = HashSet::from([NodeIndex::new(0), NodeIndex::new(1)]);
    let mut circuit = CircuitBuilder::new();
    let first = circuit.instantiate_subcircuit_named("stage1", RcStage, &interface, |o: &()| *o);
    let second = circuit.instantiate_subcircuit_named("stage2", RcStage, &interface, |o: &()| *o);
    circuit.connect(first[&NodeIndex::new(1)], second[&NodeIndex::new(0)]);
    circuit.label(first[&NodeIndex::new(0)], NetLabel::local("IN"));

    let netlist = circuit
        .build(&())
        .to_netlist()?
        .to_kicad()?
        .with_source("examples/kicad_rc_filter.rs");
    netlist.write(&mut io::stdout().lock())?;
    Ok(())
}
