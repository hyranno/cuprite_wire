//! Parts, circuits and helpers shared by the unit tests.

use std::collections::BTreeMap;

use crate::{
    circuit::{CircuitBuilder, Named, Node},
    kicad::{KicadPartInfo, KicadPinInfo, LibId},
    naming::NetLabel,
    parts::{Part, PinType},
    spice::{ModelSource, SpiceElement, SpiceNetlist, SpicePartInfo},
    subcircuit::SubcircuitGenerator,
};

#[path = "../tests/common/mod.rs"]
mod common;

pub use common::{check_golden, deck_of, simulation_error_message, strings};

/// The SPICE netlist of `circuit`.
pub fn spice_netlist(circuit: &CircuitBuilder) -> SpiceNetlist {
    circuit.build(&()).to_netlist().unwrap().to_spice().unwrap()
}

/// A 10k / 100n RC low-pass filter between the local nets `IN` and `OUT`, with `GND`.
pub fn rc_filter() -> CircuitBuilder {
    let mut circuit = CircuitBuilder::new();
    let r = circuit.instantiate_part(Named("r", Resistor("10k")));
    let c = circuit.instantiate_part(Named("c", Capacitor("100n")));
    circuit.label(r["1"], NetLabel::local("IN"));
    circuit.connect(r["2"], c["1"]);
    circuit.label(c["1"], NetLabel::local("OUT"));
    circuit.label(c["2"], NetLabel::global("GND"));
    circuit
}

/// A 1k / 1u RC low-pass stage. Node 0: input, node 1: output (labeled `OUT` inside).
#[derive(Debug, Clone)]
pub struct RcStage;

impl SubcircuitGenerator for RcStage {
    type Options = ();
    type Interface = [Node; 2];

    fn construct_subcircuit(
        &self,
        _options: &(),
        circuit: &mut CircuitBuilder<()>,
        interface: Self::Interface,
    ) {
        let [input, output] = interface;
        let r = circuit.instantiate_part(Resistor("1k"));
        let c = circuit.instantiate_part(Capacitor("1u"));
        circuit.connect(input, r["1"]);
        circuit.connect(r["2"], output);
        circuit.connect(output, c["1"]);
        circuit.label(output, NetLabel::local("OUT"));
        circuit.label(c["2"], NetLabel::global("GND"));
    }
}

fn lib_id(s: &str) -> LibId {
    s.parse().unwrap()
}

fn two_pin_kicad_info(footprint: &str, symbol: &str) -> KicadPartInfo {
    KicadPartInfo {
        footprint: Some(lib_id(footprint)),
        pins: BTreeMap::from([
            (
                "1".to_string(),
                KicadPinInfo::new("1", None, PinType::Passive),
            ),
            (
                "2".to_string(),
                KicadPinInfo::new("2", None, PinType::Passive),
            ),
        ]),
        lib_source: Some(lib_id(symbol)),
        ..Default::default()
    }
}

#[derive(Debug, Clone)]
pub struct Resistor(pub &'static str);

impl Part for Resistor {
    fn ref_prefix(&self) -> String {
        "R".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        vec!["1".into(), "2".into()]
    }
    fn value(&self) -> String {
        self.0.into()
    }
    fn kicad(&self) -> Option<KicadPartInfo> {
        Some(two_pin_kicad_info(
            "Resistor_SMD:R_0603_1608Metric",
            "Device:R",
        ))
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        Some(SpicePartInfo::element(SpiceElement::new(
            'R',
            ["1", "2"],
            self.0,
        )))
    }
}

#[derive(Debug, Clone)]
pub struct Capacitor(pub &'static str);

impl Part for Capacitor {
    fn ref_prefix(&self) -> String {
        "C".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        vec!["1".into(), "2".into()]
    }
    fn value(&self) -> String {
        self.0.into()
    }
    fn kicad(&self) -> Option<KicadPartInfo> {
        Some(two_pin_kicad_info(
            "Capacitor_SMD:C_0603_1608Metric",
            "Device:C",
        ))
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        Some(SpicePartInfo::element(SpiceElement::new(
            'C',
            ["1", "2"],
            self.0,
        )))
    }
}

/// A single op-amp in a 5-pin package.
#[derive(Debug, Clone)]
pub struct OpAmp;

impl Part for OpAmp {
    fn ref_prefix(&self) -> String {
        "U".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        ["out", "vss", "in_p", "in_n", "vdd"]
            .map(String::from)
            .to_vec()
    }
    fn value(&self) -> String {
        "OpAmp".into()
    }
    fn kicad(&self) -> Option<KicadPartInfo> {
        let pins = [
            ("out", "1", "OUT", PinType::Output),
            ("vss", "2", "V-", PinType::PowerIn),
            ("in_p", "3", "+", PinType::Input),
            ("in_n", "4", "-", PinType::Input),
            ("vdd", "5", "V+", PinType::PowerIn),
        ];
        Some(KicadPartInfo {
            footprint: Some(lib_id("Package_TO_SOT_SMD:SOT-23-5")),
            pins: pins
                .into_iter()
                .map(|(id, pad, name, pin_type)| {
                    (id.to_string(), KicadPinInfo::new(pad, Some(name), pin_type))
                })
                .collect(),
            ..Default::default()
        })
    }
    /// An ideal amplifier with a gain of 100k (the supply pins are only connected to the model).
    fn spice(&self) -> Option<SpicePartInfo> {
        Some(SpicePartInfo::element(
            SpiceElement::new('X', ["in_p", "in_n", "vdd", "vss", "out"], "ideal_opamp")
                .with_model(ModelSource::inline("ideal_opamp", IDEAL_OPAMP)),
        ))
    }
}

pub const IDEAL_OPAMP: &str = "\
.subckt ideal_opamp inp inn vdd vss out
E1 out 0 inp inn 100k
.ends ideal_opamp
";

/// An IC with three GND pads connected inside, and a pad with a name containing `/`.
#[derive(Debug, Clone)]
pub struct MultiGndIc;

impl Part for MultiGndIc {
    fn ref_prefix(&self) -> String {
        "U".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        ["vcc", "gnd_a", "gnd_b", "gnd_c", "io"]
            .map(String::from)
            .to_vec()
    }
    fn value(&self) -> String {
        "MultiGnd".into()
    }
    fn internal_connections(&self) -> Vec<Vec<String>> {
        vec![["gnd_a", "gnd_b", "gnd_c"].map(String::from).to_vec()]
    }
    fn kicad(&self) -> Option<KicadPartInfo> {
        let pins = [
            ("vcc", "1", "VCC", PinType::PowerIn),
            ("gnd_a", "2", "GND", PinType::PowerIn),
            ("gnd_b", "10", "GND", PinType::Passive),
            ("gnd_c", "11", "GND", PinType::Passive),
            ("io", "3", "I/O", PinType::Bidirectional),
        ];
        Some(KicadPartInfo {
            footprint: Some(lib_id("Package_SO:SOIC-16_3.9x9.9mm_P1.27mm")),
            pins: pins
                .into_iter()
                .map(|(id, pad, name, pin_type)| {
                    (id.to_string(), KicadPinInfo::new(pad, Some(name), pin_type))
                })
                .collect(),
            ..Default::default()
        })
    }
}

/// A part whose information is given directly, for testing invalid definitions.
#[derive(Debug, Clone)]
pub struct CustomPart {
    pub ref_prefix: &'static str,
    pub pin_ids: Vec<&'static str>,
    pub internal_connections: Vec<Vec<&'static str>>,
    /// `(pin ID, net name)`.
    pub global_labels: Vec<(&'static str, &'static str)>,
    pub kicad: Option<KicadPartInfo>,
    pub spice: Option<SpicePartInfo>,
}

impl CustomPart {
    /// A two-pin part with valid KiCad information.
    pub fn valid() -> Self {
        CustomPart {
            ref_prefix: "X",
            pin_ids: vec!["1", "2"],
            internal_connections: vec![],
            global_labels: vec![],
            kicad: Some(two_pin_kicad_info("Custom:Footprint", "Custom:Symbol")),
            spice: Some(SpicePartInfo::element(SpiceElement::new(
                'X',
                ["1", "2"],
                "custom",
            ))),
        }
    }
}

impl Part for CustomPart {
    fn ref_prefix(&self) -> String {
        self.ref_prefix.into()
    }
    fn pin_ids(&self) -> Vec<String> {
        self.pin_ids.iter().map(|s| s.to_string()).collect()
    }
    fn value(&self) -> String {
        "custom".into()
    }
    fn internal_connections(&self) -> Vec<Vec<String>> {
        self.internal_connections
            .iter()
            .map(|group| group.iter().map(|s| s.to_string()).collect())
            .collect()
    }
    fn global_labels(&self) -> Vec<(String, String)> {
        self.global_labels
            .iter()
            .map(|(pin, net)| (pin.to_string(), net.to_string()))
            .collect()
    }
    fn kicad(&self) -> Option<KicadPartInfo> {
        self.kicad.clone()
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        self.spice.clone()
    }
}
