//! Export and simulation tests of whole circuits.
//!
//! The simulation tests run the `ngspice` program, which the devcontainer provides.

use crate::{
    circuit::{CircuitBuilder, Named},
    error::CircuitErrorKind,
    naming::NetLabel,
    spice::{
        Ammeter, IndependentSource, ModelSource, Simulator, SpiceElement, SpiceOptions,
        SpicePartInfo, Table, Waveform,
        digital::{
            DigitalDriver, DigitalLoad, DigitalSequence, InputCharacteristics, Level,
            OutputCharacteristics, Pull,
        },
    },
    test_parts::{
        self, CustomPart, IDEAL_OPAMP, MultiGndIc, OpAmp, RcStage, Resistor, deck_of, rc_filter,
        simulation_error_message, spice_netlist,
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
    let r = circuit.instantiate_part(Resistor("1k"));
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
        .0
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

/// The voltage of the line in the table of [`simulate_line`].
const LINE: &str = r#"v("/LINE")"#;

/// A [`DigitalDriver`] and a [`DigitalLoad`] on the line `/LINE`, simulated for `stop` seconds;
/// the table has the voltage of the line ([`LINE`]).
fn simulate_line(
    simulator: &dyn Simulator,
    driver: DigitalDriver,
    load: DigitalLoad,
    step: f64,
    stop: f64,
) -> Table {
    let mut circuit = CircuitBuilder::new();
    let driver = circuit.instantiate_part(driver);
    let load = circuit.instantiate_part(load);
    circuit.connect(driver[DigitalDriver::OUT], load[DigitalLoad::IN]);
    circuit.label(driver[DigitalDriver::OUT], NetLabel::local("LINE"));
    circuit.label(driver[DigitalDriver::GND], NetLabel::global("GND"));
    circuit.label(load[DigitalLoad::GND], NetLabel::global("GND"));
    let deck = deck_of(&circuit).control([
        format!("tran {step:e} {stop:e}"),
        format!("wrdata line.txt {LINE}"),
    ]);
    simulator.run(&deck).unwrap().table("line.txt").unwrap()
}

/// A driver with 1 kohm output resistance into 1 nF: the rise follows the time constant (1 us),
/// and the input reads `High` when it crosses 70 % of 3.3 V.
fn simulate_digital_rc_load(simulator: &dyn Simulator) {
    let output = OutputCharacteristics::cmos(3.3).with_resistance(1e3);
    let input = InputCharacteristics::cmos(3.3).with_capacitance(1e-9);
    let sent = DigitalSequence::new(Level::Low).at(1e-6, Level::High);
    let table = simulate_line(
        simulator,
        DigitalDriver::new(output, sent),
        DigitalLoad::new(input.clone()),
        10e-9,
        6e-6,
    );
    let v = table.interpolate(LINE, 2e-6).unwrap();
    assert!((v - 3.3 * 0.632).abs() < 0.02, "v(2us) = {v}");
    let read = input.decode(table.scale(), table.column(LINE).unwrap());
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
    let table = simulate_line(
        simulator,
        DigitalDriver::new(output, sent),
        DigitalLoad::new(input.clone()),
        10e-9,
        6e-6,
    );
    let at = |t| table.interpolate(LINE, t).unwrap();
    // Driven low through 50 ohm against the pull-up.
    assert!(
        (at(0.5e-6) - 3.3 * 50.0 / 10_050.0).abs() < 1e-3,
        "{}",
        at(0.5e-6)
    );
    assert!((at(2e-6) - 3.3 * 0.632).abs() < 0.03, "{}", at(2e-6));
    assert!((at(4.9e-6) - 3.3).abs() < 0.1, "{}", at(4.9e-6));
    assert!(at(5.1e-6) < 0.05, "{}", at(5.1e-6));
    let read = input.decode(table.scale(), table.column(LINE).unwrap());
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
    let table = simulate_line(
        simulator,
        DigitalDriver::new(OutputCharacteristics::cmos(3.3), sent.clone()),
        DigitalLoad::new(input.clone()),
        100e-9,
        sent.end_time() + 20e-6,
    );
    let read = input.decode(table.scale(), table.column(LINE).unwrap());
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
