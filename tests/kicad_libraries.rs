//! Parts generated from the KiCad 9 libraries installed in the devcontainer.
//!
//! Run with `cargo test --features kicad-libraries --test kicad_libraries`.
//! The KiCad libraries are only read at compile time; the golden files are the netlists
//! of a circuit designed with them (`tests/golden/kicad_libraries.{net,cir}`).
//! Run with `UPDATE_GOLDEN=1` to (re)generate them.

use std::path::Path;

use cuprite_wire::{
    circuit::CircuitBuilder, kicad_part, naming::NetLabel, spice::IndependentSource,
};

kicad_part! {
    struct Resistor {
        symbol: "Device:R",
        footprint: "Resistor_SMD:R_0603_1608Metric",
        value: param,
    }
}

kicad_part! {
    struct Capacitor {
        symbol: "Device:C",
        footprint: "Capacitor_SMD:C_0603_1608Metric",
        value: param,
    }
}

kicad_part! {
    /// A diode with KiCad's built-in model (Sim.Device D, Sim.Pins 1=K 2=A).
    struct Diode {
        symbol: "Device:D",
        footprint: "Diode_SMD:D_SOD-123",
        value: param,
    }
}

kicad_part! {
    /// KiCad's generic dual op-amp subcircuit in Simulation_SPICE.sp.
    struct DualOpAmp {
        symbol: "Device:Opamp_Dual",
        footprint: "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm",
    }
}

kicad_part! {
    /// A multi-unit symbol derived with `extends`, without a simulation model.
    struct Lm358 {
        symbol: "Amplifier_Operational:LM358",
        footprint: "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm",
        spice: excluded,
    }
}

kicad_part! {
    /// An exposed pad without a symbol pin, connected to V- inside the part (`extra_pads`).
    struct Lm358Ep {
        symbol: "Amplifier_Operational:LM358",
        footprint: "Package_SO:SOIC-8-1EP_3.9x4.9mm_P1.27mm_EP2.41x3.3mm",
        pins: { "9" => "ep" },
        extra_pads: ["9"],
        internal_connections: [["4", "ep"]],
        spice: excluded,
    }
}

kicad_part! {
    struct Npn {
        symbol: "Simulation_SPICE:NPN",
        footprint: "Package_TO_SOT_SMD:SOT-23",
    }
}

kicad_part! {
    struct Nmos {
        symbol: "Simulation_SPICE:NMOS",
        footprint: "Package_TO_SOT_SMD:SOT-23",
    }
}

/// A circuit using every part: an op-amp stage driving a transistor, a diode and a MOSFET.
fn circuit() -> CircuitBuilder {
    let mut c = CircuitBuilder::new();
    let amp = c.instantiate_part_named("amp", Box::new(DualOpAmp));
    let spare = c.instantiate_part_named("spare", Box::new(Lm358));
    let rf = c.instantiate_part_named("rf", Box::new(Resistor::new("10k")));
    let rg = c.instantiate_part_named("rg", Box::new(Resistor::new("1k")));
    let rb = c.instantiate_part_named("rb", Box::new(Resistor::new("4.7k")));
    let rc = c.instantiate_part_named("rc", Box::new(Resistor::new("1k")));
    let cf = c.instantiate_part_named("cf", Box::new(Capacitor::new("100n")));
    let rpd = c.instantiate_part_named("rpd", Box::new(Resistor::new("100k")));
    let d = c.instantiate_part_named("d", Box::new(Diode::new("1N4148")));
    let q = c.instantiate_part_named("q", Box::new(Npn));
    let m = c.instantiate_part_named("m", Box::new(Nmos));
    let gnd = |c: &mut CircuitBuilder, node| c.label(node, NetLabel::global("GND"));
    let vcc = |c: &mut CircuitBuilder, node| c.label(node, NetLabel::global("+5V"));

    // Channel 1: non-inverting amplifier of IN with a gain of 11.
    c.label(amp["3"], NetLabel::local("IN"));
    c.connect(amp["1"], rf["1"]);
    c.connect(rf["2"], amp["2"]);
    c.connect(amp["2"], rg["1"]);
    gnd(&mut c, rg["2"]);
    c.label(amp["1"], NetLabel::local("AMP"));
    // Channel 2: a follower of ground.
    c.connect(amp["7"], amp["6"]);
    gnd(&mut c, amp["5"]);
    vcc(&mut c, amp["8"]);
    gnd(&mut c, amp["4"]);
    // The unused LM358: inputs to ground, outputs as followers, powered.
    for (out, inv, noninv) in [("1", "2", "3"), ("7", "6", "5")] {
        c.connect(spare[out], spare[inv]);
        gnd(&mut c, spare[noninv]);
    }
    vcc(&mut c, spare["8"]);
    gnd(&mut c, spare["4"]);
    // The transistor switches the load; the diode clamps it to +5V, the MOSFET pulls OUT down.
    c.connect(amp["1"], rb["1"]);
    c.connect(rb["2"], q["2"]);
    gnd(&mut c, q["3"]);
    c.connect(q["1"], rc["2"]);
    vcc(&mut c, rc["1"]);
    c.connect(q["1"], d["2"]);
    vcc(&mut c, d["1"]);
    c.label(q["1"], NetLabel::local("OUT"));
    c.connect(m["1"], q["1"]);
    c.connect(m["2"], cf["1"]);
    gnd(&mut c, cf["2"]);
    c.connect(m["2"], rpd["1"]);
    gnd(&mut c, rpd["2"]);
    gnd(&mut c, m["3"]);
    c
}

fn check_golden(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing; run with UPDATE_GOLDEN=1", path.display()));
    assert_eq!(actual, expected, "output differs from {}", path.display());
}

#[test]
fn test_golden() {
    let netlist = circuit().build(&()).to_netlist().unwrap();
    let mut out = Vec::new();
    netlist
        .to_kicad()
        .unwrap()
        .with_source("tests/kicad_libraries.rs")
        .write(&mut out)
        .unwrap();
    check_golden("kicad_libraries.net", &String::from_utf8(out).unwrap());
    let spice = netlist.to_spice().unwrap().with_title("kicad_libraries");
    check_golden("kicad_libraries.cir", &spice.to_string());
}

/// The circuit simulates with KiCad's models: the amplifier output is 11 times the input
/// minus the model's 10 mV input offset.
#[test]
fn test_simulate() {
    let mut bench = circuit();
    let supply = bench.instantiate_part_named("supply", IndependentSource::voltage(5.0));
    bench.label(supply["p"], NetLabel::global("+5V"));
    bench.label(supply["n"], NetLabel::global("GND"));
    let input = bench.instantiate_part_named("input", IndependentSource::voltage(0.2));
    bench.label(input["p"], NetLabel::local("IN"));
    bench.label(input["n"], NetLabel::global("GND"));
    let deck = bench
        .build(&())
        .to_netlist()
        .unwrap()
        .to_spice()
        .unwrap()
        .deck()
        .control([
            "op",
            r#"let amp = v("/AMP")"#,
            r#"let out = v("/OUT")"#,
            "print amp out",
        ]);
    let sim = deck.run().unwrap();
    let amp = sim.expect_value("amp");
    assert!(
        (amp - (0.2 - 0.01) * 11.0).abs() < 0.01,
        "amp = {amp}\n{}",
        sim.log()
    );
    // The transistor is on, so OUT is pulled low.
    let out = sim.expect_value("out");
    assert!(out < 1.0, "out = {out}");
}

/// The netlist of a part with an extra pad, to check that pcbnew accepts a pad that is not a
/// pin of the symbol (`tools/check_netlist_import.py tests/golden/kicad_libraries_ep.net`).
#[test]
fn test_golden_extra_pad() {
    let mut c = CircuitBuilder::new();
    let u = c.instantiate_part_named("u", Box::new(Lm358Ep));
    let r = c.instantiate_part_named("r", Box::new(Resistor::new("10k")));
    for (out, inv, noninv) in [("1", "2", "3"), ("7", "6", "5")] {
        c.connect(u[out], u[inv]);
        c.label(u[noninv], NetLabel::global("GND"));
    }
    c.connect(u["8"], r["1"]);
    c.label(u["8"], NetLabel::global("+5V"));
    c.label(r["2"], NetLabel::global("GND"));
    c.label(u["4"], NetLabel::global("GND"));
    let mut out = Vec::new();
    c.build(&())
        .to_netlist()
        .unwrap()
        .to_kicad()
        .unwrap()
        .with_source("tests/kicad_libraries.rs")
        .write(&mut out)
        .unwrap();
    check_golden("kicad_libraries_ep.net", &String::from_utf8(out).unwrap());
}
