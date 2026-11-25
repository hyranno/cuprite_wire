//! Export and simulation tests of whole circuits.
//!
//! The simulation tests run the `ngspice` program, which the devcontainer provides.

use crate::{
    circuit::{CircuitBuilder, Named},
    error::{CircuitErrorKind, CircuitWarningKind},
    naming::NetLabel,
    spice::{
        Ammeter, IndependentSource, ModelSource, Simulator, SpiceElement, SpiceOptions,
        SpicePartInfo, Waveform,
        digital::{
            DigitalDriver, DigitalLoad, DigitalSequence, InputCharacteristics, Level,
            OutputCharacteristics, Pull,
        },
    },
    test_parts::{
        self, CustomPart, IDEAL_OPAMP, Line, LineDeck, MultiGndIc, OpAmp, RcStage, Resistor,
        deck_of, rc_filter, simulation_error_message, spice_netlist,
    },
};

/// Defines a module per test function, with a test running it on each [`Simulator`]:
/// `cli` ([`Ngspice`]) and, with the `shared-ngspice` feature, `shared` ([`SharedNgspice`]).
/// The backends must give the same results.
///
/// [`SharedNgspice`]: crate::spice::SharedNgspice
macro_rules! on_each_simulator {
    ($($name:ident),* $(,)?) => {$(
        mod $name {
            #[test]
            fn cli() {
                super::$name(&crate::spice::Ngspice::default());
            }

            #[cfg(feature = "shared-ngspice")]
            #[test]
            fn shared() {
                super::$name(&crate::spice::SharedNgspice::default());
            }
        }
    )*};
}
#[cfg(feature = "ibis")]
pub(crate) use on_each_simulator;

on_each_simulator!(
    simulate_ac,
    simulate_transient,
    simulate_operating_point,
    simulate_subcircuit_model,
    simulate_pspice_model,
    simulation_error,
    aborted_transient_error,
    simulate_digital_rc_load,
    simulate_open_drain,
    simulate_uart,
    typed_analyses,
    typed_operating_point,
    typed_transient_options,
    typed_result_errors,
    typed_ac_probes,
    typed_ac_phase,
);

fn check_golden(name: &str, text: &impl ToString) {
    test_parts::check_golden(name, &text.to_string());
}

/// [`rc_filter`] driven by a voltage source on `IN`.
fn rc_filter_bench(source: IndependentSource) -> CircuitBuilder {
    let mut circuit = rc_filter();
    let v = circuit.instantiate_part(Named("source", source));
    circuit.label(v[IndependentSource::P], NetLabel::local("IN"));
    circuit.label(v[IndependentSource::N], NetLabel::global("GND"));
    circuit
}

#[test]
fn test_golden_rc_filter() {
    let netlist = spice_netlist(&rc_filter_bench(
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
    let first = circuit.instantiate_subcircuit(Named("stage1", RcStage));
    let second = circuit.instantiate_subcircuit(RcStage);
    circuit.connect(first[1], second[0]);
    let v = circuit.instantiate_part(IndependentSource::voltage(1.0));
    circuit.connect(v[IndependentSource::P], first[0]);
    circuit.label(v[IndependentSource::P], NetLabel::local("IN"));
    circuit.label(v[IndependentSource::N], NetLabel::global("GND"));
    let connector = circuit.instantiate_part(Named(
        "connector",
        CustomPart {
            ref_prefix: "J",
            spice: Some(SpicePartInfo::Excluded),
            ..CustomPart::valid()
        },
    ));
    circuit.connect(connector["1"], second[1]);
    circuit.label(connector["2"], NetLabel::global("GND"));
    check_golden("hierarchy_bench.cir", &spice_netlist(&circuit));
}

/// A dual op-amp written as two elements sharing an inline model, an included model library,
/// and unused pins.
#[derive(Debug, Clone)]
struct DualOpAmp;

impl crate::parts::Part for DualOpAmp {
    fn ref_prefix(&self) -> String {
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
                .with_model(ModelSource::include(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/models/unused.lib"
                )))
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
    let dual = circuit.instantiate_part(DualOpAmp);
    let single = circuit.instantiate_part(OpAmp);
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
    check_golden("multi_element.cir", &spice_netlist(&circuit));
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
            ref_prefix: "XX",
            ..valid()
        },
        CustomPart {
            ref_prefix: "Xx",
            ..valid()
        },
    ];
    for (i, part) in parts.into_iter().enumerate() {
        let pins = circuit.instantiate_part(part);
        circuit.label(pins["1"], NetLabel::global("GND"));
        circuit.label(pins["2"], NetLabel::local(format!("n{i}")));
    }
    // Nets merged or broken by ngspice.
    for name in ["A", "a", "é"] {
        let r = circuit.instantiate_part(Resistor("1k"));
        circuit.label(r["1"], NetLabel::local(name));
        circuit.label(r["2"], NetLabel::global("GND"));
    }
    let u = circuit.instantiate_part(MultiGndIc);
    circuit.label(u["io"], NetLabel::local("n0"));

    let netlist = circuit.build(&()).to_netlist().unwrap();
    let errors = netlist.to_spice().unwrap_err().errors;
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
    let r = circuit.instantiate_part(Resistor("1k"));
    circuit.label(r["1"], NetLabel::local("A"));
    circuit.label(r["2"], NetLabel::local("GND"));
    circuit.label(r["2"], NetLabel::local("B"));
    let errors = circuit
        .build(&())
        .to_netlist()
        .unwrap()
        .to_spice()
        .unwrap_err();
    let kinds: Vec<_> = errors.errors.iter().map(|e| e.kind.clone()).collect();
    assert_eq!(kinds, vec![CircuitErrorKind::GroundMissing]);
    // The warnings of the netlist are kept.
    let warnings: Vec<_> = errors.warnings.iter().map(|w| w.kind.clone()).collect();
    assert_eq!(
        warnings,
        vec![CircuitWarningKind::MultipleNetNames {
            used: "/B".into(),
            others: vec!["/GND".into()],
        }]
    );
}

/// Nets added by `SpiceOptions::ground_net` become the ground, also next to `GND`, by their exact
/// net names.
#[test]
fn test_extra_ground_nets() {
    let mut circuit = CircuitBuilder::new();
    let source = circuit.instantiate_part(IndependentSource::voltage(1.0));
    circuit.label(source[IndependentSource::P], NetLabel::local("IN"));
    circuit.label(source[IndependentSource::N], NetLabel::global("AGND"));
    let r = circuit.instantiate_part(Resistor("1k"));
    circuit.connect(r["1"], source[IndependentSource::P]);
    circuit.label(r["2"], NetLabel::global("GND"));
    let load = circuit.instantiate_part(Resistor("1k"));
    circuit.connect(load["1"], source[IndependentSource::P]);
    circuit.label(load["2"], NetLabel::global("agnd"));
    let netlist = circuit.build(&()).to_netlist().unwrap();

    let spice = netlist
        .to_spice_with(&SpiceOptions::new().ground_net("AGND"))
        .unwrap();
    assert_eq!(spice.node("AGND"), Some("0"));
    assert_eq!(spice.node("GND"), Some("0"));
    // Not the same net name: converted as usual, and colliding with `AGND` is not reported.
    assert_eq!(spice.node("agnd"), Some("agnd"));

    // Without the option, `AGND` and `agnd` collide.
    let kinds: Vec<_> = netlist
        .to_spice()
        .unwrap_err()
        .errors
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![CircuitErrorKind::SpiceNodeCollision {
            name: "agnd".into(),
            other: "AGND".into(),
            node: "agnd".into(),
        }]
    );

    // An extra ground alone is enough.
    let mut circuit = CircuitBuilder::new();
    let r = circuit.instantiate_part(Resistor("1k"));
    circuit.label(r["1"], NetLabel::local("A"));
    circuit.label(r["2"], NetLabel::local("AGND"));
    let netlist = circuit.build(&()).to_netlist().unwrap();
    let spice = netlist
        .to_spice_with(&SpiceOptions::new().ground_net("/AGND"))
        .unwrap();
    assert_eq!(spice.node("/AGND"), Some("0"));
}

#[test]
fn test_deck() {
    let deck = spice_netlist(&rc_filter_bench(IndependentSource::voltage(1.0)))
        .with_title("deck")
        .deck()
        .directive(".options reltol=1e-4")
        .control(["op", "print all"]);
    check_golden("deck.cir", &deck);
}

// Simulations.

/// The -3 dB frequency of the RC filter is 1 / (2 pi R C).
fn simulate_ac(simulator: &dyn Simulator) {
    let deck = deck_of(&rc_filter_bench(
        IndependentSource::voltage(0.0).with_ac(1.0),
    ))
    .control([
        "ac dec 50 1 100k",
        r#"meas ac f3db WHEN vdb("/OUT")=-3"#,
        r#"wrdata ac.txt vdb("/OUT") vp("/OUT")"#,
    ]);
    let sim = simulator.run(&deck).unwrap();
    let expected = 1.0 / (2.0 * std::f64::consts::PI * 10e3 * 100e-9);
    let f3db = sim.expect_value("f3db");
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
fn simulate_transient(simulator: &dyn Simulator) {
    let step = Waveform::Pulse {
        initial: 0.0,
        pulsed: 1.0,
        delay: 0.0,
        rise: 1e-9,
        fall: 1e-9,
        width: 1.0,
        period: 2.0,
    };
    let deck = deck_of(&rc_filter_bench(
        IndependentSource::voltage(0.0).with_waveform(step),
    ))
    .control(["tran 10u 5m", r#"wrdata tran.txt v("/OUT")"#]);
    let sim = simulator.run(&deck).unwrap();
    let table = sim.table("tran.txt").unwrap();
    // ngspice writes the header without the quotes: `v(/OUT)`.
    assert_eq!(table.names(), ["time", "v(/OUT)"]);
    let v = table.interpolate(r#"v("/OUT")"#, 1e-3).unwrap();
    assert!((v - 0.632).abs() < 0.005, "v(1ms) = {v}");
}

/// A divider with an ammeter: the operating point gives the node voltage and the current.
fn simulate_operating_point(simulator: &dyn Simulator) {
    let mut circuit = CircuitBuilder::new();
    let v = circuit.instantiate_part(IndependentSource::voltage(10.0));
    let meter = circuit.instantiate_part(Ammeter);
    let top = circuit.instantiate_part(Resistor("1k"));
    let bottom = circuit.instantiate_part(Resistor("3k"));
    circuit.connect(v[IndependentSource::P], meter[Ammeter::P]);
    circuit.connect(meter[Ammeter::N], top["1"]);
    circuit.connect(top["2"], bottom["1"]);
    circuit.label(bottom["1"], NetLabel::local("MID"));
    circuit.label(bottom["2"], NetLabel::global("GND"));
    circuit.label(v[IndependentSource::N], NetLabel::global("GND"));
    let deck = deck_of(&circuit).control([
        "op",
        r#"let mid = v("/MID")"#,
        "let current = i(VM1)",
        "print mid current",
    ]);
    let sim = simulator.run(&deck).unwrap();
    assert!(
        (sim.expect_value("mid") - 7.5).abs() < 1e-6,
        "{}",
        sim.log()
    );
    assert!(
        (sim.expect_value("current") - 2.5e-3).abs() < 1e-9,
        "{}",
        sim.log()
    );
}

/// A non-inverting amplifier with the op-amp's inline subcircuit model: gain 1 + 9k / 1k.
fn simulate_subcircuit_model(simulator: &dyn Simulator) {
    let mut circuit = CircuitBuilder::new();
    let amp = circuit.instantiate_part(OpAmp);
    let feedback = circuit.instantiate_part(Resistor("9k"));
    let ground = circuit.instantiate_part(Resistor("1k"));
    circuit.connect(amp["out"], feedback["1"]);
    circuit.connect(feedback["2"], amp["in_n"]);
    circuit.connect(amp["in_n"], ground["1"]);
    circuit.label(ground["2"], NetLabel::global("GND"));
    circuit.label(amp["out"], NetLabel::local("OUT"));
    let input = circuit.instantiate_part(IndependentSource::voltage(0.1));
    circuit.connect(input[IndependentSource::P], amp["in_p"]);
    circuit.label(input[IndependentSource::N], NetLabel::global("GND"));
    let supply = circuit.instantiate_part(IndependentSource::voltage(5.0));
    circuit.connect(supply[IndependentSource::P], amp["vdd"]);
    circuit.connect(supply[IndependentSource::N], amp["vss"]);
    circuit.label(supply[IndependentSource::N], NetLabel::global("GND"));

    let deck = deck_of(&circuit).control(["op", r#"let out = v("/OUT")"#, "print out"]);
    let sim = simulator.run(&deck).unwrap();
    let out = sim.expect_value("out");
    assert!((out - 1.0).abs() < 1e-3, "out = {out}");
}

/// A window comparator written for PSpice (`IF` and `&` work only in ngspice's PSpice mode).
const PSPICE_WINDOW: &str = ".SUBCKT WINDOW IN OUT
E1 OUT 0 VALUE = { IF(V(IN) > 1 & V(IN) < 2, 5, 0) }
.ENDS WINDOW
";

/// A model marked as PSpice makes the deck run in ngspice's PSpice mode.
fn simulate_pspice_model(simulator: &dyn Simulator) {
    let deck = |model: ModelSource| {
        let mut circuit = CircuitBuilder::new();
        let window = circuit.instantiate_part(CustomPart {
            ref_prefix: "U",
            pin_ids: vec!["in", "out"],
            internal_connections: vec![],
            global_labels: vec![],
            kicad: None,
            spice: Some(SpicePartInfo::element(
                SpiceElement::new('X', ["in", "out"], "WINDOW").with_model(model),
            )),
        });
        let input = circuit.instantiate_part(IndependentSource::voltage(1.5));
        let load = circuit.instantiate_part(Resistor("1k"));
        circuit.connect(input[IndependentSource::P], window["in"]);
        circuit.connect(window["out"], load["1"]);
        circuit.label(window["out"], NetLabel::local("OUT"));
        circuit.label(input[IndependentSource::N], NetLabel::global("GND"));
        circuit.label(load["2"], NetLabel::global("GND"));
        deck_of(&circuit).control(["op", r#"let out = v("/OUT")"#, "print out"])
    };
    let pspice = deck(ModelSource::inline("WINDOW", PSPICE_WINDOW).pspice());
    // Inline models need the mode for the whole deck (`a`), not only for included files.
    assert_eq!(pspice.netlist().ngbehavior(), Some("psa"));
    let text = pspice.to_string();
    assert!(
        text.starts_with(".title cuprite_wire\n* requires: set ngbehavior=psa (.spiceinit)\n"),
        "{text}"
    );
    let sim = simulator.run(&pspice).unwrap();
    assert!(
        (sim.expect_value("out") - 5.0).abs() < 1e-6,
        "{}",
        sim.log()
    );
    assert_eq!(
        std::fs::read_to_string(sim.dir().join(".spiceinit")).unwrap(),
        "set ngbehavior=psa\n"
    );

    let ngspice = deck(ModelSource::inline("WINDOW", PSPICE_WINDOW));
    assert_eq!(ngspice.netlist().ngbehavior(), None);
    assert!(!ngspice.to_string().contains("requires"));
    // The default mode does not read the PSpice syntax.
    let error = simulation_error_message(simulator.run(&ngspice).unwrap_err());
    assert!(error.contains("ngspice reported an error"), "{error}");
}

#[test]
fn test_run_dir_always_has_spiceinit() {
    // Without a `.spiceinit` in the run directory, the CLI would read the user's `~/.spiceinit`.
    let deck = deck_of(&rc_filter_bench(IndependentSource::voltage(1.0)));
    assert_eq!(deck.netlist().ngbehavior(), None);
    let dir = crate::spice::simulation::prepare_run_dir(&deck).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join(".spiceinit")).unwrap(),
        "unset ngbehavior\n"
    );
}

/// A deck that ngspice rejects is reported with its log.
fn simulation_error(simulator: &dyn Simulator) {
    let deck = deck_of(&rc_filter_bench(IndependentSource::voltage(1.0)))
        .control(["op", r#"wrdata x.txt v("/nothing")"#]);
    let error = simulation_error_message(simulator.run(&deck).unwrap_err());
    assert!(error.contains("no such vector"), "{error}");
}

/// A transient analysis that stops in the middle (`Timestep too small`) is an error, although
/// ngspice prints no `Error` line and exits with 0.
fn aborted_transient_error(simulator: &dyn Simulator) {
    // The source switches whenever the output crosses 0.5 V, which ngspice cannot step over.
    let element = SpiceElement::new(
        'B',
        ["1", "2"],
        "V = time < 0.2m ? 0.4 : (v(FB) > 0.5 ? 0 : 1)",
    );
    let mut circuit = CircuitBuilder::new();
    let source = circuit.instantiate_part(CustomPart {
        ref_prefix: "B",
        spice: Some(SpicePartInfo::element(element)),
        ..CustomPart::valid()
    });
    let r = circuit.instantiate_part(Resistor("1k"));
    let c = circuit.instantiate_part(test_parts::Capacitor("1n"));
    circuit.connect(source["1"], r["1"]);
    circuit.connect(r["2"], c["1"]);
    circuit.label(c["1"], NetLabel::global("FB"));
    circuit.label(source["2"], NetLabel::global("GND"));
    circuit.label(c["2"], NetLabel::global("GND"));
    let deck = deck_of(&circuit).control(["tran 1u 1m", "wrdata fb.txt v(FB)"]);
    let error = simulation_error_message(simulator.run(&deck).unwrap_err());
    assert!(error.contains("simulation(s) aborted"), "{error}");
}

/// A file no command wrote is reported with its name.
#[test]
fn test_missing_table() {
    let deck = deck_of(&rc_filter_bench(IndependentSource::voltage(1.0))).control(["op"]);
    let error = deck.run().unwrap().table("missing.txt").unwrap_err();
    assert!(
        matches!(&error, crate::spice::SimulationError::Table { file, .. } if file == "missing.txt"),
        "{error}"
    );
    assert!(
        error.to_string().starts_with("cannot read missing.txt: "),
        "{error}"
    );
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
    let first = circuit.instantiate_subcircuit(Named("stage1", RcStage));
    let second = circuit.instantiate_subcircuit(RcStage);
    let r_in = circuit.instantiate_part(Named("input", Resistor("100")));
    circuit.label(r_in["1"], NetLabel::local("I/O"));
    // An unnamed net between two parts in the root.
    let r_mid = circuit.instantiate_part(Named("mid", Resistor("220")));
    circuit.connect(r_in["2"], r_mid["1"]);
    circuit.connect(r_mid["2"], first[0]);
    circuit.connect(first[1], second[0]);
    let amp = circuit.instantiate_part(Named("amp", OpAmp));
    circuit.connect(amp["in_p"], second[1]);
    circuit.connect(amp["out"], amp["in_n"]);
    circuit.label(amp["out"], NetLabel::local("a(b) c"));
    circuit.label(amp["vdd"], NetLabel::global("+5V"));
    circuit.label(amp["vss"], NetLabel::global("GND"));
    // An unconnected pin.
    let r_open = circuit.instantiate_part(Named("open", Resistor("1Meg")));
    circuit.connect(r_open["1"], amp["out"]);
    let connector = circuit.instantiate_part(Named(
        "connector",
        CustomPart {
            ref_prefix: "J",
            spice: Some(SpicePartInfo::Excluded),
            ..CustomPart::valid()
        },
    ));
    circuit.connect(connector["1"], amp["out"]);
    circuit.label(connector["2"], NetLabel::global("GND"));

    let netlist = circuit.build(&()).to_netlist().unwrap();
    let mut kicad = Vec::new();
    netlist
        .to_kicad()
        .unwrap()
        .with_source("kicad_pair")
        .write(&mut kicad)
        .unwrap();
    test_parts::check_golden("kicad_pair.net", &String::from_utf8(kicad).unwrap());
    check_golden(
        "kicad_pair.cir",
        &netlist.to_spice().unwrap().with_title("kicad_pair"),
    );
}

/// A [`DigitalDriver`] and a [`DigitalLoad`] on the line `/LINE`, simulated for `stop` seconds.
fn simulate_line(
    simulator: &dyn Simulator,
    driver: DigitalDriver,
    load: DigitalLoad,
    step: f64,
    stop: f64,
) -> Line {
    let mut circuit = CircuitBuilder::new();
    let driver = circuit.instantiate_part(driver);
    let load = circuit.instantiate_part(load);
    circuit.connect(driver[DigitalDriver::OUT], load[DigitalLoad::IN]);
    circuit.label(driver[DigitalDriver::OUT], NetLabel::local("LINE"));
    circuit.label(driver[DigitalDriver::GND], NetLabel::global("GND"));
    circuit.label(load[DigitalLoad::GND], NetLabel::global("GND"));
    let deck = LineDeck::new(&circuit, step, stop);
    deck.read(&simulator.run(&deck.deck).unwrap())
}

/// A driver with 1 kohm output resistance into 1 nF: the rise follows the time constant (1 us),
/// and the input reads `High` when it crosses 70 % of 3.3 V.
fn simulate_digital_rc_load(simulator: &dyn Simulator) {
    let output = OutputCharacteristics::cmos(3.3).with_resistance(1e3);
    let input = InputCharacteristics::cmos(3.3).with_capacitance(1e-9);
    let sent = DigitalSequence::new(Level::Low).at(1e-6, Level::High);
    let line = simulate_line(
        simulator,
        DigitalDriver::new(output, sent),
        DigitalLoad::new(input.clone()),
        10e-9,
        6e-6,
    );
    let v = line.at(2e-6);
    assert!((v - 3.3 * 0.632).abs() < 0.02, "v(2us) = {v}");
    let read = input.decode(line.time(), line.volts());
    assert_eq!(read.initial(), Level::Low);
    let [(t, Level::High)] = read.events() else {
        panic!("{read:?}");
    };
    // 1 us + ln(1 / 0.3) us
    assert!((t - 2.204e-6).abs() < 10e-9, "{t}");
}

/// An open-drain output with a 10 kohm pull-up and 100 pF: released (`HiZ`) at 1 us, it rises
/// with the time constant of the pull-up (1 us); pulled low again at 5 us, it falls at once.
fn simulate_open_drain(simulator: &dyn Simulator) {
    let output = OutputCharacteristics::cmos(3.3);
    let input = InputCharacteristics::cmos(3.3)
        .with_capacitance(100e-12)
        .with_pull(Pull::up(3.3, 10e3));
    let sent = DigitalSequence::new(Level::Low)
        .at(1e-6, Level::HiZ)
        .at(5e-6, Level::Low);
    let line = simulate_line(
        simulator,
        DigitalDriver::new(output, sent),
        DigitalLoad::new(input.clone()),
        10e-9,
        6e-6,
    );
    let at = |t| line.at(t);
    // Driven low through 50 ohm against the pull-up.
    assert!(
        (at(0.5e-6) - 3.3 * 50.0 / 10_050.0).abs() < 1e-3,
        "{}",
        at(0.5e-6)
    );
    assert!((at(2e-6) - 3.3 * 0.632).abs() < 0.03, "{}", at(2e-6));
    assert!((at(4.9e-6) - 3.3).abs() < 0.1, "{}", at(4.9e-6));
    assert!(at(5.1e-6) < 0.05, "{}", at(5.1e-6));
    let read = input.decode(line.time(), line.volts());
    let [(rise, Level::High), (fall, Level::Low)] = read.events() else {
        panic!("{read:?}");
    };
    assert!((rise - 2.204e-6).abs() < 30e-9, "{rise}");
    assert!((fall - 5e-6).abs() < 20e-9, "{fall}");
}

/// UART frames read back at the middle of each bit.
fn simulate_uart(simulator: &dyn Simulator) {
    let baud = 115_200.0;
    let sent = DigitalSequence::new(Level::High).uart(10e-6, baud, b"cuprite!");
    let input = InputCharacteristics::cmos(3.3);
    let line = simulate_line(
        simulator,
        DigitalDriver::new(OutputCharacteristics::cmos(3.3), sent.clone()),
        DigitalLoad::new(input.clone()),
        100e-9,
        sent.end_time() + 20e-6,
    );
    let read = input.decode(line.time(), line.volts());
    for bit in 0..80 {
        let t = 10e-6 + (bit as f64 + 0.5) / baud;
        assert_eq!(read.level_at(t), sent.level_at(t), "bit {bit}");
    }
    assert_eq!(read.events().len(), sent.events().len());
}

/// The shared library gives the same table and printed values as the CLI, and its run directory
/// holds both forms of the deck.
#[cfg(feature = "shared-ngspice")]
#[test]
fn test_shared_library_matches_cli() {
    use crate::spice::{Ngspice, SharedNgspice};
    let deck = deck_of(&rc_filter_bench(IndependentSource::voltage(1.0))).control([
        "op",
        r#"print v("/OUT")"#,
        "tran 10u 2m uic",
        r#"wrdata tran.txt v("/OUT")"#,
        r#"meas tran half WHEN v("/OUT")=0.5"#,
    ]);
    let cli = Ngspice::default().run(&deck).unwrap();
    let shared = SharedNgspice::default().run(&deck).unwrap();
    assert_eq!(
        shared.table("tran.txt").unwrap(),
        cli.table("tran.txt").unwrap()
    );
    assert_eq!(shared.value("half"), cli.value("half"));
    assert!(shared.value("half").is_some(), "{}", shared.log());
    assert_eq!(shared.value("v(/out)"), Some(1.0), "{}", shared.log());
    assert_eq!(
        std::fs::read_to_string(shared.dir().join("deck.cir")).unwrap(),
        deck.to_string()
    );
    let library_deck = std::fs::read_to_string(shared.dir().join("shared.cir")).unwrap();
    let tran = shared.dir().join("tran.txt");
    assert!(
        library_deck.contains(&format!(
            r#"wrdata {} v("/OUT")"#,
            crate::spice::spice_path(&tran)
        )),
        "{library_deck}"
    );
    assert!(!library_deck.contains("quit"), "{library_deck}");
}

/// A deck without a control block runs the analyses of its dot directives in both backends.
#[cfg(feature = "shared-ngspice")]
#[test]
fn test_shared_library_runs_dot_analyses() {
    use crate::spice::{Ngspice, SharedNgspice};
    let deck = deck_of(&rc_filter_bench(IndependentSource::voltage(1.0)))
        .directive(".tran 10u 2m uic")
        .directive(".meas tran half WHEN v(/OUT)=0.5");
    let cli = Ngspice::default().run(&deck).unwrap();
    let shared = SharedNgspice::default().run(&deck).unwrap();
    assert!(cli.value("half").is_some(), "{}", cli.log());
    assert_eq!(shared.value("half"), cli.value("half"), "{}", shared.log());
}

/// `write` without a file writes `rawspice.raw` in the run directory, not in the working
/// directory of the process.
#[cfg(feature = "shared-ngspice")]
#[test]
fn test_shared_library_writes_default_raw_file() {
    use crate::spice::SharedNgspice;
    let deck = deck_of(&rc_filter_bench(IndependentSource::voltage(1.0))).control(["op", "write"]);
    let shared = SharedNgspice::default().run(&deck).unwrap();
    assert!(
        shared.dir().join("rawspice.raw").is_file(),
        "{}",
        shared.log()
    );
}

/// A measurement whose name starts with `error` is a value, not an error of ngspice.
#[test]
fn test_measurement_named_error() {
    let deck = deck_of(&rc_filter_bench(IndependentSource::voltage(1.0)))
        .control(["tran 10u 2m uic", r#"meas tran error_max MAX v("/OUT")"#]);
    let sim = deck.run().unwrap();
    assert!((sim.expect_value("error_max") - 1.0).abs() < 0.2);
}

/// After an error ngspice cannot recover from (a syntax error in an expression), the library is
/// loaded again and the next run succeeds.
#[cfg(feature = "shared-ngspice")]
#[test]
fn test_shared_library_recovers() {
    use crate::spice::SharedNgspice;
    let simulator = SharedNgspice::default();
    let deck = |args: &str| {
        let mut circuit = CircuitBuilder::new();
        let element = SpiceElement::new('B', ["1", "2"], args);
        let source = circuit.instantiate_part(CustomPart {
            ref_prefix: "B",
            spice: Some(SpicePartInfo::element(element)),
            ..CustomPart::valid()
        });
        let load = circuit.instantiate_part(Resistor("1k"));
        circuit.connect(source["1"], load["1"]);
        circuit.label(source["1"], NetLabel::local("OUT"));
        circuit.label(source["2"], NetLabel::global("GND"));
        circuit.label(load["2"], NetLabel::global("GND"));
        deck_of(&circuit).control(["op", r#"let out = v("/OUT")"#, "print out"])
    };
    let error = simulation_error_message(simulator.run(&deck("V = { 1 + * 2 }")).unwrap_err());
    assert!(error.contains("cannot recover"), "{error}");
    let sim = simulator.run(&deck("V = { 1 + 2 }")).unwrap();
    assert_eq!(sim.expect_value("out"), 3.0);
}

/// A driver whose `gnd` is 1 V above the ground drives relative to it.
#[test]
fn test_digital_driver_reference() {
    let mut circuit = CircuitBuilder::new();
    let sequence = DigitalSequence::new(Level::Low).at(1e-6, Level::High);
    let driver = circuit.instantiate_part(DigitalDriver::new(
        OutputCharacteristics::cmos(3.3),
        sequence,
    ));
    let load = circuit.instantiate_part(Resistor("1meg"));
    let offset = circuit.instantiate_part(IndependentSource::voltage(1.0));
    circuit.connect(driver[DigitalDriver::OUT], load["1"]);
    circuit.label(driver[DigitalDriver::OUT], NetLabel::local("LINE"));
    circuit.label(driver[DigitalDriver::GND], NetLabel::local("VSS"));
    circuit.label(load["2"], NetLabel::local("VSS"));
    circuit.label(offset[IndependentSource::P], NetLabel::local("VSS"));
    circuit.label(offset[IndependentSource::N], NetLabel::global("GND"));
    let deck = deck_of(&circuit).control([
        "tran 10n 2u",
        r#"let low = v("/LINE")[1]"#,
        r#"let high = v("/LINE")[length(time) - 1]"#,
        "print low high",
    ]);
    let sim = deck.run().unwrap();
    assert!(
        (sim.expect_value("low") - 1.0).abs() < 1e-3,
        "{}",
        sim.log()
    );
    assert!(
        (sim.expect_value("high") - 4.3).abs() < 1e-3,
        "{}",
        sim.log()
    );
}

/// Typed analyses mixed with string commands in one deck, read without file or column names.
fn typed_analyses(simulator: &dyn Simulator) {
    use crate::spice::analysis::{Ac, Dc, Tran};
    let deck = deck_of(&rc_filter_bench(
        IndependentSource::voltage(0.0)
            .with_ac(1.0)
            .with_waveform(Waveform::Pulse {
                initial: 0.0,
                pulsed: 1.0,
                delay: 0.0,
                rise: 1e-9,
                fall: 1e-9,
                width: 1.0,
                period: 2.0,
            }),
    ));
    let out = deck.voltage("/OUT").unwrap();
    let across = deck.voltage_between("/IN", "/OUT").unwrap();
    let current = deck.current("V1").unwrap();
    let tran = Tran::new(10e-6, 5e-3).probes([out.clone(), across.clone(), current.clone()]);
    let ac = Ac::decade(50, 1.0, 100e3).probes([out.clone(), current.clone()]);
    let dc = Dc::new(&deck.source("V1").unwrap(), 0.0, 2.0, 0.5).probe(&out);
    let deck = deck
        .analysis(&tran)
        .analysis(&ac)
        // `meas ... WHEN` takes a vector or `vdb(<node>)`, not an expression like `db(v(...))`.
        .control([r#"meas ac f3db WHEN vdb("/OUT")=-3"#])
        .analysis(&dc);
    let sim = simulator.run(&deck).unwrap();

    let t = sim.result(&tran).unwrap();
    let v = t.interpolate(&out, 1e-3).unwrap().unwrap();
    assert!((v - 0.632).abs() < 0.01, "{v}");
    let vr = t.interpolate(&across, 1e-3).unwrap().unwrap();
    assert!((v + vr - 1.0).abs() < 1e-3, "{v} + {vr}");
    let i = t.interpolate(&current, 1e-3).unwrap().unwrap();
    // The current flows out of the positive pin of the source: -(1 - v) / 10k.
    assert!((i + (1.0 - v) / 10e3).abs() < 1e-6, "{i}");

    let a = sim.result(&ac).unwrap();
    let index = a.frequency().iter().position(|&f| f >= 10e3).unwrap();
    let gain = a.get(&out).unwrap()[index].db();
    assert!((gain + 36.0).abs() < 0.2, "{gain}");
    assert!(a.get(&across).is_err());
    let f3db = sim.expect_value("f3db");
    assert!((f3db - 159.15).abs() < 2.0, "{f3db}");

    let d = sim.result(&dc).unwrap();
    assert_eq!(d.scale(), [0.0, 0.5, 1.0, 1.5, 2.0]);
    assert_eq!(d.get(&out).unwrap(), d.scale());

    // An analysis of another deck.
    assert!(matches!(
        sim.result(&Tran::new(10e-6, 5e-3).probe(&out)),
        Err(crate::spice::SimulationError::Analysis(_))
    ));
}

fn typed_operating_point(simulator: &dyn Simulator) {
    use crate::spice::analysis::{Op, ProbeError};
    let mut circuit = CircuitBuilder::new();
    let v = circuit.instantiate_part(IndependentSource::voltage(10.0));
    let meter = circuit.instantiate_part(Ammeter);
    let top = circuit.instantiate_part(Resistor("1k"));
    let bottom = circuit.instantiate_part(Resistor("3k"));
    circuit.connect(v[IndependentSource::P], meter[Ammeter::P]);
    circuit.connect(meter[Ammeter::N], top["1"]);
    circuit.connect(top["2"], bottom["1"]);
    circuit.label(bottom["1"], NetLabel::local("MID"));
    circuit.label(bottom["2"], NetLabel::global("GND"));
    circuit.label(v[IndependentSource::N], NetLabel::global("GND"));
    let deck = deck_of(&circuit);
    let mid = deck.voltage("/MID").unwrap();
    let current = deck.current("vm1").unwrap();
    assert_eq!(current.expression(), "i(VM1)");
    assert_eq!(deck.voltage("GND"), Err(ProbeError::Ground("GND".into())));
    assert_eq!(
        deck.voltage("/NONE"),
        Err(ProbeError::UnknownNet("/NONE".into()))
    );
    assert_eq!(
        deck.current("R1"),
        Err(ProbeError::UnknownVoltageSource("R1".into()))
    );
    assert_eq!(deck.source("v1").unwrap().element(), "V1");
    assert_eq!(
        deck.source("R1"),
        Err(ProbeError::UnknownSource("R1".into()))
    );
    let ground = deck.voltage_between("GND", "/MID").unwrap();
    assert_eq!(ground.expression(), r#"(-v("/MID"))"#);
    let op = Op::new().probes([mid.clone(), current.clone(), ground.clone()]);
    let sim = simulator.run(&deck.analysis(&op)).unwrap();
    let result = sim.result(&op).unwrap();
    assert!((result.get(&mid).unwrap() - 7.5).abs() < 1e-6);
    assert!((result.get(&current).unwrap() - 2.5e-3).abs() < 1e-9);
    assert!((result.get(&ground).unwrap() + 7.5).abs() < 1e-6);
}

#[test]
#[should_panic(expected = "the deck already has the analysis tran 1e-5 0.001")]
fn test_analysis_added_twice() {
    let deck = deck_of(&rc_filter_bench(IndependentSource::voltage(1.0)));
    let out = deck.voltage("/OUT").unwrap();
    let tran = crate::spice::analysis::Tran::new(10e-6, 1e-3).probe(&out);
    let _ = deck.analysis(&tran).analysis(&tran);
}

/// `with_uic` starts from the discharged capacitor instead of the operating point, `with_start`
/// records from its time on, and `with_max_step` makes ngspice take more points.
fn typed_transient_options(simulator: &dyn Simulator) {
    use crate::spice::analysis::Tran;
    let deck = deck_of(&rc_filter_bench(IndependentSource::voltage(1.0)));
    let out = deck.voltage("/OUT").unwrap();
    let from_op = Tran::new(10e-6, 2e-3).probe(&out);
    let uic = Tran::new(10e-6, 2e-3).with_uic().probe(&out);
    let late = Tran::new(10e-6, 2e-3)
        .with_start(1e-3)
        .with_uic()
        .probe(&out);
    let fine = Tran::new(10e-6, 2e-3)
        .with_max_step(1e-6)
        .with_uic()
        .probe(&out);
    let sim = simulator
        .run(
            &deck
                .analysis(&from_op)
                .analysis(&uic)
                .analysis(&late)
                .analysis(&fine),
        )
        .unwrap();

    let from_op = sim.result(&from_op).unwrap();
    assert!(
        from_op
            .get(&out)
            .unwrap()
            .iter()
            .all(|v| (v - 1.0).abs() < 1e-6)
    );
    let uic = sim.result(&uic).unwrap();
    // The capacitor starts discharged (ngspice gives 1e-4 V at the first point).
    let v0 = uic.get(&out).unwrap()[0];
    assert!(v0.abs() < 1e-3, "{v0}");
    let v = uic.interpolate(&out, 1e-3).unwrap().unwrap();
    assert!((v - 0.632).abs() < 0.01, "{v}");

    let late = sim.result(&late).unwrap();
    let first = late.scale()[0];
    assert!((1e-3..1.01e-3).contains(&first), "{first}");
    let v = late.interpolate(&out, 1.5e-3).unwrap().unwrap();
    assert!((v - 0.777).abs() < 0.01, "{v}");

    let fine = sim.result(&fine).unwrap();
    assert!(
        fine.scale().len() > 2 * uic.scale().len(),
        "{} points for {}",
        fine.scale().len(),
        uic.scale().len()
    );
    assert!(fine.scale().windows(2).all(|t| t[1] - t[0] <= 1.000_001e-6));
}

#[test]
fn test_transient_command() {
    use crate::spice::analysis::{AnalysisCommand, Tran};
    let tran = Tran::new(10e-6, 2e-3);
    assert_eq!(tran.command(), "tran 1e-5 0.002");
    assert_eq!(tran.clone().with_uic().command(), "tran 1e-5 0.002 uic");
    assert_eq!(
        tran.clone().with_start(1e-3).command(),
        "tran 1e-5 0.002 0.001"
    );
    assert_eq!(
        tran.clone().with_max_step(1e-6).with_uic().command(),
        "tran 1e-5 0.002 0 1e-6 uic"
    );
    assert_eq!(
        tran.clone().with_max_step(1e-6).with_start(1e-3).command(),
        "tran 1e-5 0.002 0.001 1e-6"
    );
    // A changed analysis is another analysis.
    let deck = deck_of(&rc_filter_bench(IndependentSource::voltage(1.0))).analysis(&tran);
    assert!(deck.analyses().contains(&tran));
    assert!(!deck.analyses().contains(&tran.clone().with_uic()));
}

/// The errors reading a typed result: an analysis without probes, a probe the analysis does not
/// record, and an output file overwritten with other columns.
fn typed_result_errors(simulator: &dyn Simulator) {
    use crate::spice::{
        SimulationError,
        analysis::{Dc, Op, Tran},
    };
    let deck = deck_of(&rc_filter_bench(IndependentSource::voltage(1.0)));
    let out = deck.voltage("/OUT").unwrap();
    let input = deck.voltage("/IN").unwrap();
    let bare = Op::new();
    let op = Op::new().probe(&out);
    let dc = Dc::new(&deck.source("V1").unwrap(), 0.0, 1.0, 0.5).probe(&out);
    let tran = Tran::new(10e-6, 1e-3).probe(&out);
    let deck = deck
        .analysis(&bare)
        .analysis(&op)
        .analysis(&dc)
        .analysis(&tran)
        // Overwrites the output of `tran` with two probes.
        .control([format!("wrdata analysis4.txt {out} {input}")]);
    let sim = simulator.run(&deck).unwrap();

    let message = |error: SimulationError| error.to_string();
    assert_eq!(
        message(sim.result(&bare).unwrap_err()),
        "cannot read the result of op: the analysis has no probes"
    );
    let op = sim.result(&op).unwrap();
    assert!((op.get(&out).unwrap() - 1.0).abs() < 1e-6);
    assert_eq!(
        message(op.get(&input).unwrap_err()),
        r#"cannot read the result of the analysis: v("/IN") is not one of its probes"#
    );
    let dc = sim.result(&dc).unwrap();
    assert!(matches!(dc.get(&input), Err(SimulationError::Analysis(_))));
    assert!(matches!(
        dc.interpolate(&input, 0.5),
        Err(SimulationError::Analysis(_))
    ));
    assert_eq!(dc.interpolate(&out, 0.25).unwrap(), Some(0.25));
    assert_eq!(dc.interpolate(&out, 2.0).unwrap(), None);
    match sim.result(&tran) {
        Err(SimulationError::Table { file, reason }) => {
            assert_eq!(file, "analysis4.txt");
            assert_eq!(reason, "3 columns for 2 (the scale and the probes)");
        }
        other => panic!("{other:?}"),
    }
}

/// The values of the voltage across the resistor and the current of the source in an AC analysis,
/// and the phase in degrees.
fn typed_ac_probes(simulator: &dyn Simulator) {
    use crate::spice::analysis::{Ac, Complex};
    let deck = deck_of(&rc_filter_bench(
        IndependentSource::voltage(0.0).with_ac(1.0),
    ));
    let out = deck.voltage("/OUT").unwrap();
    let across = deck.voltage_between("/IN", "/OUT").unwrap();
    let current = deck.current("V1").unwrap();
    let ac = Ac::decade(10, 10.0, 10e3).probes([out.clone(), across.clone(), current.clone()]);
    let sim = simulator.run(&deck.analysis(&ac)).unwrap();
    let a = sim.result(&ac).unwrap();
    assert_eq!(a.frequency().len(), 31);
    // `wrdata` writes 9 significant digits.
    let close = |x: Complex, y: Complex| (x.re - y.re).hypot(x.im - y.im) < 1e-8 * y.norm();
    for (i, &f) in a.frequency().iter().enumerate() {
        // H = 1 / (1 + jωRC) with RC = 1 ms.
        let wrc = std::f64::consts::TAU * f * 1e-3;
        let denominator = 1.0 + wrc * wrc;
        let expected = Complex {
            re: 1.0 / denominator,
            im: -wrc / denominator,
        };
        let v = a.get(&out).unwrap()[i];
        assert!(close(v, expected), "{f}: {v:?} for {expected:?}");
        let vr = a.get(&across).unwrap()[i];
        assert!(close(
            Complex {
                re: v.re + vr.re,
                im: v.im + vr.im
            },
            Complex { re: 1.0, im: 0.0 }
        ));
        // The current flows out of the positive pin of the source: -vr / 10k.
        let current = a.get(&current).unwrap()[i];
        let expected = Complex {
            re: -vr.re / 10e3,
            im: -vr.im / 10e3,
        };
        assert!(
            close(current, expected),
            "{f}: {current:?} for {expected:?}"
        );
        assert!((v.phase_degrees() + wrc.atan().to_degrees()).abs() < 1e-6);
    }
}

/// The phases and the group delay of three RC stages (whose phase goes down to -3π/2) match
/// `vp()`, `cph()` and `group_delay()` of ngspice.
fn typed_ac_phase(simulator: &dyn Simulator) {
    use crate::spice::analysis::Ac;
    let mut circuit = CircuitBuilder::new();
    let stages =
        ["s1", "s2", "s3"].map(|name| circuit.instantiate_subcircuit(Named(name, RcStage)));
    circuit.connect(stages[0][1], stages[1][0]);
    circuit.connect(stages[1][1], stages[2][0]);
    let v = circuit.instantiate_part(IndependentSource::voltage(0.0).with_ac(1.0));
    circuit.connect(v[IndependentSource::P], stages[0][0]);
    circuit.label(v[IndependentSource::N], NetLabel::global("GND"));
    let deck = deck_of(&circuit);
    let out = deck.voltage("/s3/OUT").unwrap();
    let ac = Ac::decade(50, 1.0, 100e3).probe(&out);
    let deck = deck.analysis(&ac).control([format!(
        r#"wrdata ngspice_phase.txt vp("/s3/OUT") cph({out}) group_delay({out})"#
    )]);
    let sim = simulator.run(&deck).unwrap();
    let a = sim.result(&ac).unwrap();
    let table = sim.table("ngspice_phase.txt").unwrap();
    let column = |i: usize| table.column(&table.names()[i]).unwrap();
    assert_eq!(column(0), a.frequency());

    let values = a.get(&out).unwrap();
    for (value, vp) in values.iter().zip(column(1)) {
        assert!((value.phase() - vp).abs() < 1e-6, "{value:?}: {vp}");
    }
    let phase = a.continuous_phase(&out).unwrap();
    for (phase, cph) in phase.iter().zip(column(2)) {
        assert!((phase - cph).abs() < 1e-6, "{phase}: {cph}");
    }
    let last = *phase.last().unwrap();
    assert!(last < -std::f64::consts::PI, "{last}");

    // The group delay goes from 6 ms (RC of 1 ms each: R1C1 + R2C2 + R3C3 + R1C2 + R1C3 + R2C3)
    // down to nearly 0 as the phase settles at -3π/2.
    let delay = a.group_delay(&out).unwrap();
    assert!((delay[0] - 6e-3).abs() < 1e-5, "{}", delay[0]);
    assert!(
        delay.last().unwrap().abs() < 1e-7,
        "{}",
        delay.last().unwrap()
    );
    // ngspice differs at the ends, where it takes a difference of lower order.
    let n = delay.len();
    for (i, (delay, ngspice)) in delay.iter().zip(column(3)).enumerate().take(n - 1).skip(1) {
        assert!(
            (delay - ngspice).abs() < 1e-2 * ngspice.abs() + 1e-9,
            "{}: {delay} for {ngspice}",
            a.frequency()[i]
        );
    }
}

/// The probes that cannot be made.
#[test]
fn test_probe_errors() {
    use crate::spice::analysis::ProbeError;
    let mut circuit = rc_filter_bench(IndependentSource::voltage(1.0));
    let r = circuit.instantiate_part(Resistor("1k"));
    circuit.label(r["1"], NetLabel::local("a$b"));
    circuit.label(r["2"], NetLabel::global("GND"));
    let deck = deck_of(&circuit);
    assert_eq!(
        deck.voltage_between("GND", "GND"),
        Err(ProbeError::Ground("GND".into()))
    );
    assert_eq!(
        deck.voltage_between("/OUT", "/NONE"),
        Err(ProbeError::UnknownNet("/NONE".into()))
    );
    assert_eq!(
        deck.voltage_between("/NONE", "GND"),
        Err(ProbeError::UnknownNet("/NONE".into()))
    );
    assert_eq!(
        deck.voltage_between("/IN", "/OUT").unwrap().expression(),
        r#"v("/IN")-v("/OUT")"#
    );
    let unsupported = ProbeError::UnsupportedNode {
        net: "/a$b".into(),
        node: "/a$b".into(),
    };
    assert_eq!(deck.voltage("/a$b"), Err(unsupported.clone()));
    assert_eq!(deck.voltage_between("/OUT", "/a$b"), Err(unsupported));
}
