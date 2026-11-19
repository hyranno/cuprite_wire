use std::collections::HashSet;

use petgraph::graph::NodeIndex;

use super::*;
use crate::{
    basic::{OpAmpSot23, Resistor0603},
    circuit::CircuitBuilder,
    naming::NetLabel,
    subcircuit::SubcircuitGenerator,
    test_parts::rc_filter,
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

/// A voltage follower. Interface nodes: 0 = input, 1 = output, 2 = positive supply, 3 = ground.
#[derive(Debug, Clone)]
struct Follower;

impl SubcircuitGenerator for Follower {
    type Options = ();

    fn construct_subcircuit(&self, _options: &()) -> CircuitBuilder {
        let mut circuit = CircuitBuilder::new();
        let [input, output, vcc, gnd] = [(); 4].map(|_| circuit.add_node());
        let u = circuit.instantiate_part_named("u", Box::new(OpAmpSot23::new("OPA")));
        let load = circuit.instantiate_part_named("load", Box::new(Resistor0603::new("10k")));
        circuit.connect(input, u["in_p"]);
        circuit.connect(output, u["out"]);
        circuit.connect(u["out"], u["in_n"]);
        circuit.connect(u["out"], load["1"]);
        circuit.connect(load["2"], gnd);
        circuit.connect(u["vcc"], vcc);
        circuit.connect(u["vee"], gnd);
        circuit
    }
}

fn follower_interface() -> HashSet<NodeIndex> {
    (0..4).map(NodeIndex::new).collect()
}

/// A [`Follower`] with labeled supplies (`VCC` / `GND`) and its input driven from outside.
fn follower_circuit(power_flags: bool) -> CircuitBuilder {
    let mut circuit = CircuitBuilder::new();
    let nodes =
        circuit.instantiate_subcircuit_named("f", Follower, &follower_interface(), |_: &()| ());
    let [input, output, vcc, gnd] = [0, 1, 2, 3].map(|i| nodes[&NodeIndex::new(i)]);
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
            error("net \"VCC\": U1.vcc (power_in) is not connected to anything"),
        ]
    );
    // The trace goes through the subcircuit instantiation to the part.
    assert_eq!(report.violations[0].diagnostic.trace.0.len(), 2);
}

#[test]
fn test_unconnected_pin() {
    let report = erc(&rc_filter());
    assert_eq!(
        summary(&report),
        vec![
            error("net \"/IN\": R1.1 (passive) is not connected to anything"),
            error("net \"GND\": C1.2 (passive) is not connected to anything"),
        ]
    );
    assert!(report.has_errors());
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
            error("net \"out\": R1.1 (passive) is not connected to anything"),
            warning("net names \"out\" and \"OUT\" differ only in case"),
        ]
    );
}

#[test]
fn test_severities() {
    let settings =
        ErcSettings::default().with_severity(ErcRule::PinNotConnected, Severity::Warning);
    let report = erc_with(&rc_filter(), &(), &settings);
    assert!(!report.has_errors());
    assert_eq!(report.warnings().count(), 2);
    report.assert_no_errors();

    let settings = settings.with_severity(ErcRule::PinNotConnected, Severity::Ignore);
    erc_with(&rc_filter(), &(), &settings).assert_clean();
}

#[test]
fn test_check_subcircuit() {
    let settings = ErcSettings::default();
    let node = NodeIndex::new;
    let ports = [
        (node(0), PinType::Output),
        (node(1), PinType::Input),
        (node(2), PinType::PowerOut),
        (node(3), PinType::PowerOut),
    ];
    check_subcircuit(&Follower, &(), &ports, &settings)
        .unwrap()
        .assert_clean();

    // Without a driver for the input.
    let report = check_subcircuit(&Follower, &(), &ports[1..], &settings).unwrap();
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
    let mut ports = ports;
    ports[1].1 = PinType::Output;
    let report = check_subcircuit(&Follower, &(), &ports, &settings).unwrap();
    assert_eq!(
        summary(&report),
        vec![error(
            "net \"Net-(U1--)\": pins of type output and output are connected: U1.out (output) and virtual pin \"port 1\" (output)"
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
    let text = report.to_string();
    assert!(
        text.starts_with("error: net \"/IN\": R1.1 (passive) is not connected to anything\n  at src/test_parts.rs:"),
        "{text}"
    );
}
