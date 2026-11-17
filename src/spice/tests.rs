//! Export tests of whole circuits.

use std::collections::HashSet;

use petgraph::graph::NodeIndex;

use crate::{
    circuit::CircuitBuilder,
    error::CircuitErrorKind,
    naming::NetLabel,
    spice::{IndependentSource, ModelSource, SpiceElement, SpiceNetlist, SpicePartInfo},
    test_parts::{self, CustomPart, IDEAL_OPAMP, MultiGndIc, OpAmp, RcStage, Resistor, rc_filter},
};

fn export(circuit: &CircuitBuilder) -> SpiceNetlist {
    circuit.build(&()).to_netlist().unwrap().to_spice().unwrap()
}

fn check_golden(name: &str, text: &impl ToString) {
    test_parts::check_golden(name, &text.to_string());
}

fn interface() -> HashSet<NodeIndex> {
    HashSet::from([NodeIndex::new(0), NodeIndex::new(1)])
}

/// [`rc_filter`] driven by a voltage source on `IN`.
fn rc_filter_bench(source: Box<IndependentSource>) -> CircuitBuilder {
    let mut circuit = rc_filter();
    let v = circuit.instantiate_part_named("source", source);
    circuit.label(v["p"], NetLabel::local("IN"));
    circuit.label(v["n"], NetLabel::global("GND"));
    circuit
}

#[test]
fn test_golden_rc_filter() {
    let netlist = export(&rc_filter_bench(
        IndependentSource::voltage(0.0).with_ac(1.0),
    ))
    .with_title("rc_filter");
    check_golden("rc_filter_bench.cir", &netlist);
    assert_eq!(netlist.node("/OUT"), Some("/OUT"));
    assert_eq!(netlist.node("GND"), Some("0"));
    assert_eq!(netlist.node("/nothing"), None);
}

/// Node names in subcircuits (including an automatic instance name with `#`), and parts excluded
/// from the simulation.
#[test]
fn test_golden_hierarchy() {
    let mut circuit = CircuitBuilder::new();
    let first = circuit.instantiate_subcircuit_named("stage1", RcStage, &interface(), |o: &()| *o);
    let second = circuit.instantiate_subcircuit(RcStage, &interface(), |o: &()| *o);
    circuit.connect(first[&NodeIndex::new(1)], second[&NodeIndex::new(0)]);
    let v = circuit.instantiate_part(IndependentSource::voltage(1.0));
    circuit.connect(v["p"], first[&NodeIndex::new(0)]);
    circuit.label(v["p"], NetLabel::local("IN"));
    circuit.label(v["n"], NetLabel::global("GND"));
    let connector = circuit.instantiate_part_named(
        "connector",
        Box::new(CustomPart {
            ref_base_name: "J",
            spice: Some(SpicePartInfo::Excluded),
            ..CustomPart::valid()
        }),
    );
    circuit.connect(connector["1"], second[&NodeIndex::new(1)]);
    circuit.label(connector["2"], NetLabel::global("GND"));
    check_golden("hierarchy_bench.cir", &export(&circuit));
}

/// A dual op-amp written as two elements sharing an inline model, an included model library,
/// and unused pins.
#[derive(Debug, Clone)]
struct DualOpAmp;

impl crate::parts::Part for DualOpAmp {
    fn get_ref_base_name(&self) -> String {
        "U".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        [
            "out_a", "in_n_a", "in_p_a", "vss", "in_p_b", "in_n_b", "out_b", "vdd", "nc",
        ]
        .map(String::from)
        .to_vec()
    }
    fn value(&self) -> String {
        "DualOpAmp".into()
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        let unit = |suffix, inp, inn, out| {
            SpiceElement::new('X', [inp, inn, "vdd", "vss", out], "ideal_opamp")
                .with_suffix(suffix)
                .with_model(ModelSource::inline("ideal_opamp", IDEAL_OPAMP))
                .with_model(ModelSource::include("/opt/models/unused.lib"))
        };
        Some(
            SpicePartInfo::elements([
                unit("A", "in_p_a", "in_n_a", "out_a"),
                unit("B", "in_p_b", "in_n_b", "out_b"),
            ])
            .with_unused_pins(["nc"]),
        )
    }
}

#[test]
fn test_golden_multi_element() {
    let mut circuit = CircuitBuilder::new();
    let dual = circuit.instantiate_part(Box::new(DualOpAmp));
    let single = circuit.instantiate_part(Box::new(OpAmp));
    for (from, to) in [("out_a", "in_n_a"), ("out_b", "in_n_b")] {
        circuit.connect(dual[from], dual[to]);
    }
    circuit.connect(dual["out_a"], dual["in_p_b"]);
    circuit.connect(dual["out_b"], single["in_p"]);
    circuit.connect(single["out"], single["in_n"]);
    circuit.label(dual["in_p_a"], NetLabel::local("IN"));
    circuit.label(single["out"], NetLabel::local("OUT"));
    for (pin, net) in [("vdd", "+5V"), ("vss", "GND")] {
        circuit.label(dual[pin], NetLabel::global(net));
        circuit.label(single[pin], NetLabel::global(net));
    }
    check_golden("multi_element.cir", &export(&circuit));
}

#[test]
fn test_spice_errors_are_collected() {
    let mut circuit = CircuitBuilder::new();
    let valid = CustomPart::valid;
    let spice = |element: SpiceElement| Some(SpicePartInfo::element(element));
    let parts = [
        // X1: no SPICE information.
        CustomPart {
            spice: None,
            ..valid()
        },
        // X2: invalid arguments.
        CustomPart {
            spice: spice(SpiceElement::new('X', ["1", "2"], "a;b")),
            ..valid()
        },
        // X3: pin 2 is not used.
        CustomPart {
            spice: spice(SpiceElement::new('X', ["1"], "m")),
            ..valid()
        },
        // X4, X5: the same model defined differently.
        CustomPart {
            spice: spice(
                SpiceElement::new('X', ["1", "2"], "m")
                    .with_model(ModelSource::inline("M", ".model M D")),
            ),
            ..valid()
        },
        CustomPart {
            spice: spice(
                SpiceElement::new('X', ["1", "2"], "m")
                    .with_model(ModelSource::inline("m", ".model m R")),
            ),
            ..valid()
        },
        // XX1 and Xx1: element names differing only in case.
        CustomPart {
            ref_base_name: "XX",
            ..valid()
        },
        CustomPart {
            ref_base_name: "Xx",
            ..valid()
        },
    ];
    for (i, part) in parts.into_iter().enumerate() {
        let pins = circuit.instantiate_part(Box::new(part));
        circuit.label(pins["1"], NetLabel::global("GND"));
        circuit.label(pins["2"], NetLabel::local(format!("n{i}")));
    }
    // Nets merged or broken by ngspice.
    for name in ["A", "a", "é"] {
        let r = circuit.instantiate_part(Box::new(Resistor("1k")));
        circuit.label(r["1"], NetLabel::local(name));
        circuit.label(r["2"], NetLabel::global("GND"));
    }
    let u = circuit.instantiate_part(Box::new(MultiGndIc));
    circuit.label(u["io"], NetLabel::local("n0"));

    let netlist = circuit.build(&()).to_netlist().unwrap();
    let errors = netlist.to_spice().unwrap_err().0;
    let kinds: Vec<_> = errors.iter().map(|e| e.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            CircuitErrorKind::SpiceUnsupported {
                ref_name: "U1".into()
            },
            CircuitErrorKind::SpiceUnsupported {
                ref_name: "X1".into()
            },
            CircuitErrorKind::InvalidSpiceElement {
                ref_name: "X2".into(),
                reason: "arguments \"a;b\" contain a comment character (';' or '$')".into(),
            },
            CircuitErrorKind::SpicePinUnmapped {
                ref_name: "X3".into(),
                pin_id: "2".into(),
            },
            CircuitErrorKind::SpiceModelConflict { name: "m".into() },
            CircuitErrorKind::SpiceNodeCollision {
                name: "/a".into(),
                other: "/A".into(),
                node: "/a".into(),
            },
            CircuitErrorKind::InvalidSpiceNodeName {
                name: "/é".into(),
                reason: "'é' is not ASCII (ngspice replaces it, which can merge nets)".into(),
            },
            CircuitErrorKind::SpiceElementCollision {
                name: "Xx1".into(),
                other: "XX1".into(),
            },
        ]
    );
    // Duplicates point at both places.
    assert!(errors[4..].iter().all(|e| e.trace.innermost().is_some()));
    assert_eq!(errors[4].notes.len(), 1);
}

#[test]
fn test_ground_is_required() {
    let mut circuit = CircuitBuilder::new();
    let r = circuit.instantiate_part(Box::new(Resistor("1k")));
    circuit.label(r["1"], NetLabel::local("A"));
    circuit.label(r["2"], NetLabel::local("GND"));
    let errors = circuit
        .build(&())
        .to_netlist()
        .unwrap()
        .to_spice()
        .unwrap_err()
        .0;
    let kinds: Vec<_> = errors.iter().map(|e| e.kind.clone()).collect();
    assert_eq!(kinds, vec![CircuitErrorKind::GroundMissing]);
}

#[test]
fn test_parts_without_spice_model_mix_with_kicad() {
    // The same circuit exports to both formats, and the SPICE node names follow the KiCad net names.
    let circuit = rc_filter_bench(IndependentSource::voltage(1.0));
    let netlist = circuit.build(&()).to_netlist().unwrap();
    let spice = netlist.to_spice().unwrap();
    for net in &netlist.nets {
        assert!(spice.node(&net.name).is_some(), "{}", net.name);
    }
    // The source has no KiCad information.
    assert!(netlist.to_kicad().is_err());
    // The plain filter has both.
    let netlist = rc_filter().build(&()).to_netlist().unwrap();
    assert!(netlist.to_kicad().is_ok());
}
