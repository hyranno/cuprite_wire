use petgraph::graph::NodeIndex;

use super::*;
use crate::{
    basic::{OpAmpSot23, Resistor0603},
    circuit::CircuitBuilder,
    kicad::{KicadPartInfo, KicadPinInfo},
    naming::NetLabel,
    subcircuit::{Interface, SubcircuitGenerator},
    test_parts::{CustomPart, rc_filter},
};

fn erc_with<B>(circuit: &CircuitBuilder<B>, options: &B, settings: &ErcSettings) -> ErcReport {
    circuit.build(options).to_netlist().unwrap().erc(settings)
}

fn erc(circuit: &CircuitBuilder) -> ErcReport {
    erc_with(circuit, &(), &ErcSettings::default())
}

fn summary(report: &ErcReport) -> Vec<(Severity, String)> {
    report
        .violations
        .iter()
        .map(|v| (v.severity, v.kind().to_string()))
        .collect()
}

fn error(message: &str) -> (Severity, String) {
    (Severity::Error, message.to_string())
}

fn warning(message: &str) -> (Severity, String) {
    (Severity::Warning, message.to_string())
}

/// A voltage follower.
#[derive(Debug, Clone)]
struct Follower;

#[derive(Interface)]
struct FollowerPorts<T = NodeIndex> {
    input: T,
    output: T,
    vcc: T,
    gnd: T,
}

impl SubcircuitGenerator for Follower {
    type Options = ();
    type Interface = FollowerPorts;

    fn construct_subcircuit(
        &self,
        _options: &(),
        circuit: &mut CircuitBuilder,
        interface: Self::Interface,
    ) {
        let FollowerPorts {
            input,
            output,
            vcc,
            gnd,
        } = interface;
        let u = circuit.instantiate_part_named("u", Box::new(OpAmpSot23::new("OPA")));
        let load = circuit.instantiate_part_named("load", Box::new(Resistor0603::new("10k")));
        circuit.connect(input, u["in_p"]);
        circuit.connect(output, u["out"]);
        circuit.connect(u["out"], u["in_n"]);
        circuit.connect(u["out"], load["1"]);
        circuit.connect(load["2"], gnd);
        circuit.connect(u["vcc"], vcc);
        circuit.connect(u["vee"], gnd);
    }
}

/// A [`Follower`] with labeled supplies (`VCC` / `GND`) and its input driven from outside.
fn follower_circuit(power_flags: bool) -> CircuitBuilder {
    let mut circuit = CircuitBuilder::new();
    let FollowerPorts {
        input,
        output,
        vcc,
        gnd,
    } = circuit.instantiate_subcircuit_named("f", Follower);
    circuit.virtual_pin(input, PinType::Output, "signal");
    circuit.label(output, NetLabel::global("OUT"));
    circuit.label(vcc, NetLabel::global("VCC"));
    circuit.label(gnd, NetLabel::global("GND"));
    if power_flags {
        circuit.power_flag(vcc);
        circuit.power_flag(gnd);
    }
    circuit
}

#[test]
fn test_clean_circuit() {
    erc(&follower_circuit(true)).assert_clean();
}

#[test]
fn test_power_pins_need_a_power_flag() {
    let report = erc(&follower_circuit(false));
    assert_eq!(
        summary(&report),
        vec![
            error(
                "net \"GND\": U1.vee (power_in) is not driven by any power_out pin (add a `power_flag` if it is powered from outside)"
            ),
            error(
                "net \"VCC\": U1.vcc (power_in) is not driven by any power_out pin (add a `power_flag` if it is powered from outside)"
            ),
            warning("label \"VCC\" is connected only to U1.vcc (power_in)"),
        ]
    );
    // The trace goes through the subcircuit instantiation to the part.
    assert_eq!(report.violations[0].diagnostic.trace.0.len(), 2);
}

#[test]
fn test_label_single_pin() {
    // A labeled pin is reported as such instead of as not connected.
    let report = erc(&rc_filter());
    assert_eq!(
        summary(&report),
        vec![
            warning("label \"/IN\" is connected only to R1.1 (passive)"),
            warning("label \"GND\" is connected only to C1.2 (passive)"),
        ]
    );
    // Traced to the label, with a note at the pin.
    let diagnostic = &report.violations[0].diagnostic;
    assert_eq!(diagnostic.trace.0[0].line(), 48);
    assert_eq!(diagnostic.notes[0].message, "R1.1 (passive) is here");
    assert_eq!(diagnostic.notes[0].trace.0[0].line(), 46);

    // A no-connect mark or an allow mark silences it.
    let mut circuit = CircuitBuilder::new();
    let r = circuit.instantiate_part(Box::new(Resistor0603::new("1k")));
    circuit.label(r["1"], NetLabel::global("A"));
    circuit.label(r["2"], NetLabel::global("B"));
    circuit.no_connect(r["1"]);
    assert_eq!(
        summary(&erc(&circuit)),
        vec![warning("label \"B\" is connected only to R1.2 (passive)")]
    );
    circuit.erc_allow(r["2"], ErcRule::LabelSinglePin);
    erc(&circuit).assert_clean();
}

/// A part with power pins named `VCC`, `GND` and `AGND`, and a passive `GND` pin.
fn ground_pins_part() -> CustomPart {
    let pins = [
        ("vcc", "1", "VCC", PinType::PowerIn),
        ("gnd", "2", "GND", PinType::PowerIn),
        ("agnd", "3", "AGND", PinType::PowerIn),
        ("shield", "4", "GND", PinType::Passive),
    ];
    CustomPart {
        ref_base_name: "U",
        pin_ids: pins.iter().map(|pin| pin.0).collect(),
        internal_connections: vec![],
        kicad: Some(KicadPartInfo {
            pins: pins
                .into_iter()
                .map(|(id, pad, name, pin_type)| {
                    (id.to_string(), KicadPinInfo::new(pad, Some(name), pin_type))
                })
                .collect(),
            ..Default::default()
        }),
        spice: None,
    }
}

#[test]
fn test_ground_pin_not_ground() {
    let build = |ground: &str| {
        let mut circuit = CircuitBuilder::new();
        let u = circuit.instantiate_part(Box::new(ground_pins_part()));
        circuit.label(u["vcc"], NetLabel::global("VCC"));
        circuit.power_flag(u["vcc"]);
        circuit.label(u["gnd"], NetLabel::global(ground));
        circuit.power_flag(u["gnd"]);
        // The passive pin is not checked.
        circuit.connect(u["agnd"], u["vcc"]);
        circuit.connect(u["shield"], u["vcc"]);
        (circuit, u)
    };
    let (mut circuit, u) = build("GND");
    let report = erc(&circuit);
    assert_eq!(
        summary(&report),
        vec![warning(
            "ground pin \"AGND\" U1.agnd (power_in) is on net \"VCC\", which is not a ground net, while another power pin of the part is on one"
        )]
    );
    circuit.erc_allow(u["agnd"], ErcRule::GroundPinNotGround);
    erc(&circuit).assert_clean();
    // Nothing is reported if no power pin of the part is on a ground net.
    erc(&build("0V").0).assert_clean();
}

#[test]
fn test_ground_names() {
    for name in [
        "GND",
        "agnd",
        "/sub/GND_A",
        "EARTH",
        "Earth_1",
        "VSS",
        "vssa",
    ] {
        assert!(is_ground_name(name), "{name}");
    }
    for name in ["0V", "/sub/EARTH", "VSSD", "VCC", "VEE"] {
        assert!(!is_ground_name(name), "{name}");
    }
}

#[test]
fn test_conflicting_outputs() {
    let mut circuit = follower_circuit(true);
    let u = circuit.instantiate_part(Box::new(OpAmpSot23::new("OPA")));
    circuit.label(u["out"], NetLabel::global("OUT"));
    circuit.connect(u["out"], u["in_n"]);
    circuit.label(u["in_p"], NetLabel::global("OUT"));
    circuit.label(u["vcc"], NetLabel::global("VCC"));
    circuit.label(u["vee"], NetLabel::global("GND"));
    let report = erc(&circuit);
    assert_eq!(
        summary(&report),
        vec![error(
            "net \"OUT\": pins of type output and output are connected: U1.out (output) and U2.out (output)"
        )]
    );
    let notes = &report.violations[0].diagnostic.notes;
    assert_eq!(notes[0].message, "U2.out (output) is here");

    // The pin map can allow it.
    let mut settings = ErcSettings::default();
    settings
        .pin_map
        .set(PinType::Output, PinType::Output, Severity::Ignore);
    erc_with(&circuit, &(), &settings).assert_clean();

    // The severity of the rule caps that of the pin map.
    let settings = ErcSettings::default().with_severity(ErcRule::PinToPin, Severity::Warning);
    let report = erc_with(&circuit, &(), &settings);
    assert_eq!(report.warnings().count(), 1);
    report.assert_no_errors();
    let settings = settings.with_severity(ErcRule::PinToPin, Severity::Ignore);
    erc_with(&circuit, &(), &settings).assert_clean();

    // The net can allow it.
    circuit.erc_allow(u["out"], ErcRule::PinToPin);
    erc(&circuit).assert_clean();
}

#[test]
fn test_overlapping_internal_connections() {
    // Output pins 4, 5 and ep connected inside the part by two overlapping groups.
    let pins = [
        ("4", PinType::Output),
        ("5", PinType::Output),
        ("ep", PinType::Output),
        ("in", PinType::Input),
    ];
    let part = CustomPart {
        ref_base_name: "U",
        pin_ids: pins.iter().map(|pin| pin.0).collect(),
        internal_connections: vec![vec!["4", "5"], vec!["4", "ep"]],
        kicad: Some(KicadPartInfo {
            pins: pins
                .into_iter()
                .enumerate()
                .map(|(i, (id, pin_type))| {
                    let pad = (i + 1).to_string();
                    (id.to_string(), KicadPinInfo::new(pad, None, pin_type))
                })
                .collect(),
            ..Default::default()
        }),
        spice: None,
    };
    let mut circuit = CircuitBuilder::new();
    let u = circuit.instantiate_part(Box::new(part));
    circuit.connect(u["in"], u["5"]);
    // The three pins are one stack, so they neither conflict nor count as several drivers.
    erc(&circuit).assert_clean();
}

/// A resistor whose second pin is left open on purpose. Node 0: the first pin.
#[derive(Debug, Clone)]
struct OpenResistor;

impl SubcircuitGenerator for OpenResistor {
    type Options = ();
    type Interface = [NodeIndex; 1];

    fn construct_subcircuit(
        &self,
        _options: &(),
        circuit: &mut CircuitBuilder,
        interface: Self::Interface,
    ) {
        let r = circuit.instantiate_part_named("r", Box::new(Resistor0603::new("1k")));
        circuit.connect(interface[0], r["1"]);
        circuit.erc_allow(r["2"], ErcRule::PinNotConnected);
    }
}

#[test]
fn test_allow() {
    let mut circuit = CircuitBuilder::new();
    let [a] = circuit.instantiate_subcircuit_named("a", OpenResistor);
    let [b] = circuit.instantiate_subcircuit_named("b", OpenResistor);
    // Allowing another rule has no effect.
    circuit.erc_allow(a, ErcRule::PinNotDriven);
    // The marks inside the subcircuits allow only the nets they are on.
    assert_eq!(
        summary(&erc(&circuit)),
        vec![
            error("net \"unconnected-(R1-Pad1)\": R1.1 (passive) is not connected to anything"),
            error("net \"unconnected-(R2-Pad1)\": R2.1 (passive) is not connected to anything"),
        ]
    );
    circuit.connect(a, b);
    erc(&circuit).assert_clean();
    // Nothing is reported for a mark without violations, or on a net without part pins.
    let node = circuit.add_node();
    circuit.erc_allow(node, ErcRule::NoConnectDangling);
    erc(&circuit).assert_clean();

    // The marks apply to a subcircuit checked alone too.
    let report = check_subcircuit(&OpenResistor, &(), &[None], &ErcSettings::default()).unwrap();
    assert_eq!(
        summary(&report),
        vec![error(
            "net \"unconnected-(R1-Pad1)\": R1.1 (passive) is not connected to anything"
        )]
    );
}

#[test]
fn test_allow_net_name_rules() {
    let mut circuit = follower_circuit(true);
    let r = circuit.instantiate_part(Box::new(Resistor0603::new("1k")));
    circuit.connect(r["1"], r["2"]);
    circuit.label(r["1"], NetLabel::global("out"));
    circuit.label(r["2"], NetLabel::local("SIG"));
    assert_eq!(
        summary(&erc(&circuit)),
        vec![
            warning("net names \"out\" and \"OUT\" differ only in case"),
            warning("net \"out\" is also named \"/SIG\""),
        ]
    );
    circuit.erc_allow(r["1"], ErcRule::SimilarNetNames);
    circuit.erc_allow(r["1"], ErcRule::MultipleNetNames);
    erc(&circuit).assert_clean();

    // Either net of similar names allows it.
    let mut circuit = follower_circuit(true);
    let r = circuit.instantiate_part(Box::new(Resistor0603::new("1k")));
    circuit.connect(r["1"], r["2"]);
    circuit.label(r["1"], NetLabel::global("out"));
    let output = circuit.add_node();
    circuit.label(output, NetLabel::global("OUT"));
    circuit.erc_allow(output, ErcRule::SimilarNetNames);
    erc(&circuit).assert_clean();
}

#[test]
fn test_no_connect_marks() {
    let mut circuit = CircuitBuilder::new();
    let r1 = circuit.instantiate_part(Box::new(Resistor0603::new("1k")));
    let r2 = circuit.instantiate_part(Box::new(Resistor0603::new("1k")));
    circuit.connect(r1["2"], r2["1"]);
    circuit.no_connect(r1["1"]);
    circuit.no_connect(r2["1"]);
    let dangling = circuit.add_node();
    circuit.no_connect(dangling);
    let report = erc(&circuit);
    assert_eq!(
        summary(&report),
        vec![
            warning(
                "net \"Net-(R1-Pad2)\" is marked no-connect but connects R1.2 (passive), R2.1 (passive)"
            ),
            error("net \"unconnected-(R2-Pad2)\": R2.2 (passive) is not connected to anything"),
            warning("no-connect mark is not on any pin"),
        ]
    );
}

#[test]
fn test_dangling_virtual_pin_and_label() {
    let mut circuit = follower_circuit(true);
    let node = circuit.add_node();
    circuit.virtual_pin(node, PinType::Input, "probe");
    let node = circuit.add_node();
    circuit.label(node, NetLabel::local("NOWHERE"));
    assert_eq!(
        summary(&erc(&circuit)),
        vec![
            warning("virtual pin \"probe\" is not connected to any part pin"),
            warning("net \"/NOWHERE\" has no part pins"),
        ]
    );
}

#[test]
fn test_similar_net_names() {
    let mut circuit = follower_circuit(true);
    let r = circuit.instantiate_part(Box::new(Resistor0603::new("1k")));
    circuit.label(r["1"], NetLabel::global("out"));
    circuit.label(r["2"], NetLabel::global("GND"));
    let report = erc(&circuit);
    assert_eq!(
        summary(&report),
        vec![
            warning("label \"out\" is connected only to R1.1 (passive)"),
            warning("net names \"out\" and \"OUT\" differ only in case"),
        ]
    );
}

#[test]
fn test_severities() {
    let report = erc(&rc_filter());
    assert_eq!(report.warnings().count(), 2);
    report.assert_no_errors();

    let settings = ErcSettings::default().with_severity(ErcRule::LabelSinglePin, Severity::Error);
    let report = erc_with(&rc_filter(), &(), &settings);
    assert!(report.has_errors());
    assert_eq!(report.errors().count(), 2);

    let settings = settings.with_severity(ErcRule::LabelSinglePin, Severity::Ignore);
    erc_with(&rc_filter(), &(), &settings).assert_clean();
}

#[test]
fn test_check_subcircuit() {
    let settings = ErcSettings::default();
    let mut ports = FollowerPorts {
        input: Some(PinType::Output),
        output: Some(PinType::Input),
        vcc: Some(PinType::PowerOut),
        gnd: Some(PinType::PowerOut),
    };
    check_subcircuit(&Follower, &(), &ports, &settings)
        .unwrap()
        .assert_clean();

    // Without a driver for the input.
    ports.input = None;
    let report = check_subcircuit(&Follower, &(), &ports, &settings).unwrap();
    assert_eq!(
        summary(&report),
        vec![
            error(
                "net \"unconnected-(U1-+-Pad3)\": input pin U1.in_p (input) is not driven by any output pin"
            ),
            error("net \"unconnected-(U1-+-Pad3)\": U1.in_p (input) is not connected to anything"),
        ]
    );
    // A port of the wrong type.
    ports.input = Some(PinType::Output);
    ports.output = Some(PinType::Output);
    let report = check_subcircuit(&Follower, &(), &ports, &settings).unwrap();
    assert_eq!(
        summary(&report),
        vec![error(
            "net \"Net-(U1--)\": pins of type output and output are connected: U1.out (output) and virtual pin \"port output\" (output)"
        )]
    );
    // The virtual pin points at the caller.
    let trace = &report.violations[0].diagnostic.notes[0].trace;
    assert_eq!(trace.0.len(), 1);
    assert_eq!(trace.0[0].file(), file!());
}

#[test]
fn test_display() {
    let report = erc(&rc_filter());
    // The source files are written with `\\` on Windows.
    let text = report.to_string().replace('\\', "/");
    assert!(
        text.starts_with(
            "warning: label \"/IN\" is connected only to R1.1 (passive)\n  at src/test_parts.rs:"
        ),
        "{text}"
    );
}
