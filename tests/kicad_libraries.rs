//! Parts generated from the KiCad libraries (9 or 10) installed in the devcontainer.
//!
//! Run with `cargo test --features kicad-libraries --test kicad_libraries`.
//! The KiCad libraries are only read at compile time; the golden files are the netlists
//! of a circuit designed with them (`tests/golden/kicad_libraries.{net,cir}`).
//! Run with `UPDATE_GOLDEN=1` to (re)generate them.

mod common;

use common::{check_golden, deck_of};
use cuprite_wire::{
    circuit::{CircuitBuilder, Named},
    kicad_part,
    naming::NetLabel,
    spice::{
        IndependentSource,
        analysis::{Op, Tran},
    },
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
    /// KiCad's generic op-amp subcircuit in Simulation_SPICE.sp (pins 1 +, 2 -, 3 V+, 4 V-, 5 out).
    struct OpAmp {
        symbol: "Simulation_SPICE:OPAMP",
        footprint: "Package_TO_SOT_SMD:SOT-23-5",
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

kicad_part! {
    /// Duplicate pins (KiCad 10): pad 6 is K of unit 1 and A of unit 2 (pin IDs `6` and `6#2`),
    /// and pad 3 is K of unit 3 and A of unit 4 (`3` and `3#2`). No simulation model.
    struct Bat54sdw {
        symbol: "Diode:BAT54SDW",
        spice: excluded,
    }
}

kicad_part! {
    /// Duplicate pins in one unit: pad 6 is the outputs Y4 and ~{P1} (`6` and `6#2`).
    struct Sn74278 {
        symbol: "74xx_IEEE:74278",
        footprint: "Package_DIP:DIP-14_W7.62mm",
        extra_pads: ["5", "11"],
        spice: excluded,
    }
}

kicad_part! {
    /// A sine source (Sim.Params `dc=0 ampl=1 f=1k ac=1`).
    struct Vsin {
        symbol: "Simulation_SPICE:VSIN",
        footprint: none,
    }
}

kicad_part! {
    /// A pulse source (Sim.Params `y1=0 y2=1 td=2n tr=2n tf=2n tw=50n per=100n`).
    struct Ipulse {
        symbol: "Simulation_SPICE:IPULSE",
        footprint: none,
    }
}

kicad_part! {
    /// A piecewise linear source.
    struct Vpwl {
        symbol: "Simulation_SPICE:VPWL",
        footprint: none,
    }
}

kicad_part! {
    /// A DC source (`Sim.Type DC`, no Sim.Params): the value.
    struct Vdc {
        symbol: "Simulation_SPICE:VDC",
        footprint: none,
        value: param,
    }
}

/// A circuit using every part: an op-amp stage driving a transistor, a diode and a MOSFET.
fn circuit() -> CircuitBuilder {
    let mut c = CircuitBuilder::new();
    let amp = c.instantiate_part(Named("amp", OpAmp));
    let spare = c.instantiate_part(Named("spare", Lm358));
    let rf = c.instantiate_part(Named("rf", Resistor::new("10k")));
    let rg = c.instantiate_part(Named("rg", Resistor::new("1k")));
    let rb = c.instantiate_part(Named("rb", Resistor::new("4.7k")));
    let rc = c.instantiate_part(Named("rc", Resistor::new("1k")));
    let cf = c.instantiate_part(Named("cf", Capacitor::new("100n")));
    let rpd = c.instantiate_part(Named("rpd", Resistor::new("100k")));
    let d = c.instantiate_part(Named("d", Diode::new("1N4148")));
    let q = c.instantiate_part(Named("q", Npn));
    let m = c.instantiate_part(Named("m", Nmos));
    let gnd = |c: &mut CircuitBuilder, node| c.label(node, NetLabel::global("GND"));
    let vcc = |c: &mut CircuitBuilder, node| c.label(node, NetLabel::global("+5V"));

    // A non-inverting amplifier of IN with a gain of 11.
    c.label(amp["1"], NetLabel::local("IN"));
    c.connect(amp["5"], rf["1"]);
    c.connect(rf["2"], amp["2"]);
    c.connect(amp["2"], rg["1"]);
    gnd(&mut c, rg["2"]);
    c.label(amp["5"], NetLabel::local("AMP"));
    vcc(&mut c, amp["3"]);
    gnd(&mut c, amp["4"]);
    // The unused LM358: inputs to ground, outputs as followers, powered.
    for (out, inv, noninv) in [("1", "2", "3"), ("7", "6", "5")] {
        c.connect(spare[out], spare[inv]);
        gnd(&mut c, spare[noninv]);
    }
    vcc(&mut c, spare["8"]);
    gnd(&mut c, spare["4"]);
    // The transistor switches the load; the diode clamps it to +5V, the MOSFET pulls OUT down.
    c.connect(amp["5"], rb["1"]);
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
    let supply = bench.instantiate_part(Named("supply", IndependentSource::voltage(5.0)));
    bench.label(supply[IndependentSource::P], NetLabel::global("+5V"));
    bench.label(supply[IndependentSource::N], NetLabel::global("GND"));
    let input = bench.instantiate_part(Named("input", IndependentSource::voltage(0.2)));
    bench.label(input[IndependentSource::P], NetLabel::local("IN"));
    bench.label(input[IndependentSource::N], NetLabel::global("GND"));
    let deck = deck_of(&bench);
    let [amp, out] = ["/AMP", "/OUT"].map(|net| deck.voltage(net).unwrap());
    let op = Op::new().probes([amp.clone(), out.clone()]);
    let sim = deck.analysis(&op).run().unwrap();
    let result = sim.result(&op).unwrap();
    let amp = result.get(&amp).unwrap();
    assert!(
        (amp - (0.2 - 0.01) * 11.0).abs() < 0.01,
        "amp = {amp}\n{}",
        sim.log()
    );
    // The transistor is on, so OUT is pulled low.
    let out = result.get(&out).unwrap();
    assert!(out < 1.0, "out = {out}");
}

/// The netlist of a part with an extra pad, to check that pcbnew accepts a pad that is not a
/// pin of the symbol (`tools/check_netlist_import.py tests/golden/kicad_libraries_ep.net`).
#[test]
fn test_golden_extra_pad() {
    let mut c = CircuitBuilder::new();
    let u = c.instantiate_part(Named("u", Lm358Ep));
    let r = c.instantiate_part(Named("r", Resistor::new("10k")));
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

/// The netlist of parts with duplicate pins: one node per pad, with the first pin's name and type.
/// `tools/compare_kicad_cli_netlist.py --library-symbols` agrees on the nodes, but for the extra
/// pads 5 and 11 (not pins of the symbol) and the single node of pad 3, whose duplicate pins
/// it leaves unconnected (KiCad then writes a net for each pin).
#[test]
fn test_golden_duplicate_pins() {
    let mut c = CircuitBuilder::new();
    let d = c.instantiate_part(Named("d", Bat54sdw));
    let u = c.instantiate_part(Named("u", Sn74278));
    let r = c.instantiate_part(Named("r", Resistor::new("10k")));
    // Two diodes in series from IN to OUT, the midpoint (pad 6) through R to a 74278 output.
    c.label(d["1"], NetLabel::local("IN"));
    c.connect(d["6#2"], r["1"]);
    c.label(d["2"], NetLabel::local("OUT"));
    c.connect(r["2"], u["6"]);
    // The other pair (pad 3) left unconnected, and the 74278 powered with its inputs grounded.
    c.label(d["4"], NetLabel::local("IN2"));
    c.label(d["5"], NetLabel::local("OUT2"));
    c.label(u["14"], NetLabel::global("+5V"));
    for pin in ["1", "2", "3", "4", "7", "12", "13", "5", "11"] {
        c.label(u[pin], NetLabel::global("GND"));
    }
    let mut out = Vec::new();
    c.build(&())
        .to_netlist()
        .unwrap()
        .to_kicad()
        .unwrap()
        .with_source("tests/kicad_libraries.rs")
        .write(&mut out)
        .unwrap();
    check_golden(
        "kicad_libraries_duplicate_pins.net",
        &String::from_utf8(out).unwrap(),
    );
}

/// The sources of Simulation_SPICE are written as kicad-cli writes them, and simulate.
#[test]
fn test_sources() {
    let mut c = CircuitBuilder::new();
    let sin = c.instantiate_part(Named("sin", Vsin));
    let pulse = c.instantiate_part(Named("pulse", Ipulse));
    let pwl = c.instantiate_part(Named("pwl", Vpwl));
    let dc = c.instantiate_part(Named("dc", Vdc::new("2.5")));
    for (name, source) in [("SIN", &sin), ("PULSE", &pulse), ("PWL", &pwl), ("DC", &dc)] {
        let r = c.instantiate_part(Named(&format!("r_{name}"), Resistor::new("1k")));
        c.label(source["1"], NetLabel::local(name));
        c.connect(source["1"], r["1"]);
        c.label(source["2"], NetLabel::global("GND"));
        c.label(r["2"], NetLabel::global("GND"));
    }
    let deck = deck_of(&c);
    let text = deck.to_string();
    for line in [
        "V1 /SIN 0 DC 0 SIN( 0 1 1k 0 0 0 ) AC 1",
        "I1 /PULSE 0 PULSE( 0 1 2n 2n 2n 50n 100n )",
        "V2 /PWL 0 PWL( 0 -1 50n -1 51n 0 97n 1 171n -1 200n -1 )",
        "V3 /DC 0 DC 2.5",
    ] {
        assert!(text.lines().any(|l| l == line), "{line}\n{text}");
    }
    let probes = ["/SIN", "/PULSE", "/PWL", "/DC"].map(|net| deck.voltage(net).unwrap());
    let tran = Tran::new(1e-9, 300e-9).probes(probes.clone());
    let sim = deck.analysis(&tran).run().unwrap();
    let result = sim.result(&tran).unwrap();
    // The current of 1 A flows from + through the source to -, so the pulse is -1 kV across R.
    let sin = (2.0 * std::f64::consts::PI * 1e3 * 250e-9).sin();
    let expect = [
        ("sin", 250e-9, sin),
        ("pulse", 130e-9, -1000.0),
        ("pwl", 97e-9, 1.0),
        ("dc", 100e-9, 2.5),
    ];
    for (probe, (name, time, value)) in probes.iter().zip(expect) {
        let actual = result.interpolate(probe, time).unwrap().unwrap();
        assert!(
            (actual - value).abs() < 1e-6 * value.abs().max(1.0),
            "{name} = {actual}\n{}",
            sim.log()
        );
    }
}
