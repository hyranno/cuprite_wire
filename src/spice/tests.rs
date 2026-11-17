//! Export and simulation tests of whole circuits.
//!
//! The simulation tests run ngspice (see [`Ngspice::from_env`]); they need `python3` and
//! `libngspice`, which the devcontainer provides.

use std::collections::HashSet;

use petgraph::graph::NodeIndex;

use crate::{
    circuit::CircuitBuilder,
    error::CircuitErrorKind,
    naming::NetLabel,
    spice::{
        Ammeter, IndependentSource, ModelSource, SpiceElement, SpiceNetlist, SpicePartInfo,
        Waveform,
    },
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
fn test_deck() {
    let deck = export(&rc_filter_bench(IndependentSource::voltage(1.0)))
        .with_title("deck")
        .deck()
        .directive(".options reltol=1e-4")
        .control(["op", "print all"]);
    check_golden("deck.cir", &deck);
}

// Simulations.

/// The -3 dB frequency of the RC filter is 1 / (2 pi R C).
#[test]
fn test_simulate_ac() {
    let deck = export(&rc_filter_bench(
        IndependentSource::voltage(0.0).with_ac(1.0),
    ))
    .deck()
    .control([
        "ac dec 50 1 100k",
        r#"meas ac f3db WHEN vdb("/OUT")=-3"#,
        r#"wrdata ac.txt vdb("/OUT") vp("/OUT")"#,
    ]);
    let sim = deck.run().unwrap();
    let expected = 1.0 / (2.0 * std::f64::consts::PI * 10e3 * 100e-9);
    let f3db = sim.value("f3db").unwrap();
    assert!(
        (f3db / expected - 1.0).abs() < 0.01,
        "f3db = {f3db}, expected {expected}"
    );
    let table = sim.table("ac.txt").unwrap();
    assert_eq!(
        table.names(),
        ["frequency", r#"vdb("/OUT")"#, r#"vp("/OUT")"#]
    );
    let gain_at_10k = table.interpolate(r#"vdb("/OUT")"#, 10e3).unwrap();
    // One decade above the corner: about -36 dB (20 log10(1 / sqrt(1 + (10k/159)^2))).
    assert!((gain_at_10k + 36.0).abs() < 0.2, "{gain_at_10k}");
}

/// The step response of the RC filter reaches 63.2% after one time constant (1 ms).
#[test]
fn test_simulate_transient() {
    let step = Waveform::Pulse {
        initial: 0.0,
        pulsed: 1.0,
        delay: 0.0,
        rise: 1e-9,
        fall: 1e-9,
        width: 1.0,
        period: 2.0,
    };
    let deck = export(&rc_filter_bench(
        IndependentSource::voltage(0.0).with_waveform(step),
    ))
    .deck()
    .control(["tran 10u 5m", r#"wrdata tran.txt v("/OUT")"#]);
    let sim = deck.run().unwrap();
    let table = sim.table("tran.txt").unwrap();
    // ngspice writes the header without the quotes: `v(/OUT)`.
    assert_eq!(table.names(), ["time", "v(/OUT)"]);
    let v = table.interpolate(r#"v("/OUT")"#, 1e-3).unwrap();
    assert!((v - 0.632).abs() < 0.005, "v(1ms) = {v}");
}

/// A divider with an ammeter: the operating point gives the node voltage and the current.
#[test]
fn test_simulate_operating_point() {
    let mut circuit = CircuitBuilder::new();
    let v = circuit.instantiate_part(IndependentSource::voltage(10.0));
    let meter = circuit.instantiate_part(Box::new(Ammeter));
    let top = circuit.instantiate_part(Box::new(Resistor("1k")));
    let bottom = circuit.instantiate_part(Box::new(Resistor("3k")));
    circuit.connect(v["p"], meter["p"]);
    circuit.connect(meter["n"], top["1"]);
    circuit.connect(top["2"], bottom["1"]);
    circuit.label(bottom["1"], NetLabel::local("MID"));
    circuit.label(bottom["2"], NetLabel::global("GND"));
    circuit.label(v["n"], NetLabel::global("GND"));
    let deck = export(&circuit).deck().control([
        "op",
        r#"let mid = v("/MID")"#,
        "let current = i(VM1)",
        "print mid current",
    ]);
    let sim = deck.run().unwrap();
    assert!(
        (sim.value("mid").unwrap() - 7.5).abs() < 1e-6,
        "{}",
        sim.log()
    );
    assert!(
        (sim.value("current").unwrap() - 2.5e-3).abs() < 1e-9,
        "{}",
        sim.log()
    );
}

/// A non-inverting amplifier with the op-amp's inline subcircuit model: gain 1 + 9k / 1k.
#[test]
fn test_simulate_subcircuit_model() {
    let mut circuit = CircuitBuilder::new();
    let amp = circuit.instantiate_part(Box::new(OpAmp));
    let feedback = circuit.instantiate_part(Box::new(Resistor("9k")));
    let ground = circuit.instantiate_part(Box::new(Resistor("1k")));
    circuit.connect(amp["out"], feedback["1"]);
    circuit.connect(feedback["2"], amp["in_n"]);
    circuit.connect(amp["in_n"], ground["1"]);
    circuit.label(ground["2"], NetLabel::global("GND"));
    circuit.label(amp["out"], NetLabel::local("OUT"));
    let input = circuit.instantiate_part(IndependentSource::voltage(0.1));
    circuit.connect(input["p"], amp["in_p"]);
    circuit.label(input["n"], NetLabel::global("GND"));
    let supply = circuit.instantiate_part(IndependentSource::voltage(5.0));
    circuit.connect(supply["p"], amp["vdd"]);
    circuit.connect(supply["n"], amp["vss"]);
    circuit.label(supply["n"], NetLabel::global("GND"));

    let deck = export(&circuit)
        .deck()
        .control(["op", r#"let out = v("/OUT")"#, "print out"]);
    let sim = deck.run().unwrap();
    let out = sim.value("out").unwrap();
    assert!((out - 1.0).abs() < 1e-3, "out = {out}");
}

/// A deck that ngspice rejects is reported with its log.
#[test]
fn test_simulation_error() {
    let deck = export(&rc_filter_bench(IndependentSource::voltage(1.0)))
        .deck()
        .control(["op", r#"wrdata x.txt v("/nothing")"#]);
    let error = deck.run().unwrap_err().to_string();
    assert!(error.contains("no such vector"), "{error}");
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

/// A circuit exported to both formats, for comparing the SPICE output with KiCad's
/// (`tools/compare_kicad_cli_netlist.py --spice tests/golden/kicad_pair.net`): node names from
/// labels with characters KiCad replaces, unnamed and unconnected nets, subcircuits with named and
/// automatic instance names, a subcircuit model, and a part excluded from the simulation.
#[test]
fn test_golden_kicad_pair() {
    let mut circuit = CircuitBuilder::new();
    let first = circuit.instantiate_subcircuit_named("stage1", RcStage, &interface(), |o: &()| *o);
    let second = circuit.instantiate_subcircuit(RcStage, &interface(), |o: &()| *o);
    let r_in = circuit.instantiate_part_named("input", Box::new(Resistor("100")));
    circuit.label(r_in["1"], NetLabel::local("I/O"));
    // An unnamed net between two parts in the root.
    let r_mid = circuit.instantiate_part_named("mid", Box::new(Resistor("220")));
    circuit.connect(r_in["2"], r_mid["1"]);
    circuit.connect(r_mid["2"], first[&NodeIndex::new(0)]);
    circuit.connect(first[&NodeIndex::new(1)], second[&NodeIndex::new(0)]);
    let amp = circuit.instantiate_part_named("amp", Box::new(OpAmp));
    circuit.connect(amp["in_p"], second[&NodeIndex::new(1)]);
    circuit.connect(amp["out"], amp["in_n"]);
    circuit.label(amp["out"], NetLabel::local("a(b) c"));
    circuit.label(amp["vdd"], NetLabel::global("+5V"));
    circuit.label(amp["vss"], NetLabel::global("GND"));
    // An unconnected pin.
    let r_open = circuit.instantiate_part_named("open", Box::new(Resistor("1Meg")));
    circuit.connect(r_open["1"], amp["out"]);
    let connector = circuit.instantiate_part_named(
        "connector",
        Box::new(CustomPart {
            ref_base_name: "J",
            spice: Some(SpicePartInfo::Excluded),
            ..CustomPart::valid()
        }),
    );
    circuit.connect(connector["1"], amp["out"]);
    circuit.label(connector["2"], NetLabel::global("GND"));

    let netlist = circuit.build(&()).to_netlist().unwrap();
    let mut kicad = Vec::new();
    netlist.to_kicad().unwrap().with_source("kicad_pair").write(&mut kicad).unwrap();
    test_parts::check_golden("kicad_pair.net", &String::from_utf8(kicad).unwrap());
    check_golden("kicad_pair.cir", &netlist.to_spice().unwrap().with_title("kicad_pair"));
}
