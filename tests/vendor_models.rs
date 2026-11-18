//! Parts made from real vendor models, which cannot be committed (their licenses restrict
//! redistribution). Opt-in: download the models, put them into one directory and run
//!
//! ```sh
//! CUPRITE_VENDOR_MODELS_DIR=/path/to/models cargo test --features vendor-models --test vendor_models
//! ```
//!
//! Expected files, as they are in TI's zip files (checked on 2026-11-18):
//! - `TPS3899DL_PSPICE_TRANS.lib` from <https://www.ti.com/lit/zip/slvmdi2> (TPS3899DL PSpice
//!   transient model, Rev. A): the `.subckt` `TPS3899DL_TRANS` with the ports
//!   `CTR CTS GND RESET_B SENSE VDD` and `PARAMS: VTH=1.5` (skidl discussion #293).
//! - `lmx58_lm2904.lib` from <https://www.ti.com/lit/zip/snom268c> (LMx58/LM2904 PSpice model,
//!   Rev. C): the single amplifier `LMX58_LM2904` with the ports `IN+ IN- VCC VEE OUT`.
//!
//! If a newer model renames something, the compile error lists the names and ports the library has.
//! The tests check the behavior, not golden files. The LM358 part also needs the KiCad libraries.

use cuprite_wire::{
    basic::{Capacitor0603, Resistor0603},
    circuit::CircuitBuilder,
    kicad_part,
    naming::NetLabel,
    parts::Part,
    spice::{IndependentSource, SpiceDialect, SpicePartInfo, Waveform},
    spice_part,
};

spice_part! {
    /// TPS3899DL with TI's model; the reset output is renamed to the datasheet's name.
    struct Tps3899 {
        library: "${CUPRITE_VENDOR_MODELS_DIR}/TPS3899DL_PSPICE_TRANS.lib",
        name: "TPS3899DL_TRANS",
        pins: { "RESET_B" => "reset_n" },
    }
}

kicad_part! {
    /// The case of skidl discussion #293: the test symbol `test:VSUP` has the pins 1-6
    /// `CTR CTS GND VDD SENSE ~{RESET_N}`, TI's model the ports `CTR CTS GND RESET_B SENSE VDD`.
    struct Tps3899Symbol {
        symbol: "test:VSUP",
        sim: {
            library: "${CUPRITE_VENDOR_MODELS_DIR}/TPS3899DL_PSPICE_TRANS.lib",
            name: "TPS3899DL_TRANS",
            pins: { "1" => "CTR", "2" => "CTS", "3" => "GND", "4" => "VDD", "5" => "SENSE", "6" => "RESET_B" },
        },
        symbol_dir: "crates/kicad_lib/tests/fixtures/symbols",
        footprint_dir: "crates/kicad_lib/tests/fixtures/footprints",
    }
}

kicad_part! {
    /// KiCad's LM358 symbol with TI's single-amplifier model used for both units.
    struct Lm358 {
        symbol: "Amplifier_Operational:LM358",
        footprint: none,
        sim: [
            { library: "${CUPRITE_VENDOR_MODELS_DIR}/lmx58_lm2904.lib", name: "LMX58_LM2904", suffix: "A",
              pins: { "3" => "IN+", "2" => "IN-", "8" => "VCC", "4" => "VEE", "1" => "OUT" } },
            { library: "${CUPRITE_VENDOR_MODELS_DIR}/lmx58_lm2904.lib", name: "LMX58_LM2904", suffix: "B",
              pins: { "5" => "IN+", "6" => "IN-", "8" => "VCC", "4" => "VEE", "7" => "OUT" } },
        ],
    }
}

/// Runs the circuit with the deck's `directives` and `commands`, and returns the values printed for `names`.
fn run(circuit: CircuitBuilder, directives: &[&str], commands: &[String], names: &[&str]) -> Vec<f64> {
    let mut deck = circuit.build(&()).to_netlist().unwrap().to_spice().unwrap().deck();
    for directive in directives {
        deck = deck.directive(*directive);
    }
    let sim = deck.control(commands.iter().cloned()).run().unwrap_or_else(|e| panic!("{e}"));
    names
        .iter()
        .map(|name| sim.value(name).unwrap_or_else(|| panic!("{}", sim.log())))
        .collect()
}

/// TI's test bench of the model: VDD ramps up to 3.3 V, SENSE drops from 3.3 V to 0.5 V (below
/// VTH = 1.5 V) between 0.5 ms and 0.75 ms, RESET is pulled up with 1k, CTR and CTS have 10 pF.
/// RESET is high except while SENSE is low.
#[test]
fn test_tps3899() {
    let mut c = CircuitBuilder::new();
    let u = c.instantiate_part(Box::new(Tps3899::new()));
    let vdd = c.instantiate_part(IndependentSource::voltage(0.0).with_waveform(Waveform::Pwl(vec![
        (0.0, 0.0),
        (66e-6, 3.3),
        (1e-3, 3.3),
    ])));
    let sense = c.instantiate_part(IndependentSource::voltage(0.0).with_waveform(Waveform::Pwl(vec![
        (0.0, 3.3),
        (500e-6, 3.3),
        (501e-6, 0.5),
        (750e-6, 0.5),
        (751e-6, 3.3),
    ])));
    let pullup = c.instantiate_part(Box::new(Resistor0603::new("1k")));
    let ctr = c.instantiate_part(Box::new(Capacitor0603::new("10p")));
    let cts = c.instantiate_part(Box::new(Capacitor0603::new("10p")));
    c.connect(vdd["p"], u["VDD"]);
    c.connect(sense["p"], u["SENSE"]);
    c.connect(pullup["1"], u["VDD"]);
    c.connect(pullup["2"], u[Tps3899::RESET_N]);
    c.connect(ctr["1"], u["CTR"]);
    c.connect(cts["1"], u["CTS"]);
    for pin in [vdd["n"], sense["n"], ctr["2"], cts["2"], u["GND"]] {
        c.label(pin, NetLabel::global("GND"));
    }
    c.label(u[Tps3899::RESET_N], NetLabel::local("RESET"));
    let times = [400e-6, 600e-6, 740e-6, 950e-6];
    let mut commands = vec!["tran 1u 1m".to_string()];
    for (i, t) in times.iter().enumerate() {
        commands.push(format!("meas tran r{i} find v(\"/RESET\") at={t}"));
    }
    let reset = run(c, &[], &commands, &["r0", "r1", "r2", "r3"]);
    let high = |v: f64| v > 3.0;
    let low = |v: f64| v < 0.4;
    assert!(high(reset[0]) && low(reset[1]) && low(reset[2]) && high(reset[3]), "RESET at {times:?}: {reset:?}");
}

/// The nodes follow the `.subckt` ports, and the model is marked as PSpice.
#[test]
fn test_tps3899_symbol() {
    let Some(SpicePartInfo::Elements { elements, .. }) = Tps3899Symbol::new().spice() else { panic!() };
    assert_eq!(elements[0].nodes, ["1", "2", "3", "6", "5", "4"]);
    assert_eq!(elements[0].models[0].dialect(), SpiceDialect::Pspice);
}

/// Both units of the LM358 work as followers. Without `rshunt`, ngspice's operating point fails
/// (gmin and source stepping), and the transient operating point it falls back to leaves the
/// outputs above the supply.
#[test]
fn test_lm358_units() {
    let mut c = CircuitBuilder::new();
    let u = c.instantiate_part(Box::new(Lm358::new()));
    let supply = c.instantiate_part(IndependentSource::voltage(5.0));
    let a = c.instantiate_part(IndependentSource::voltage(1.0));
    let b = c.instantiate_part(IndependentSource::voltage(2.0));
    c.connect(supply["p"], u["8"]);
    c.connect(a["p"], u["3"]);
    c.connect(b["p"], u["5"]);
    c.connect(u["1"], u["2"]);
    c.connect(u["7"], u["6"]);
    for pin in [supply["n"], a["n"], b["n"], u["4"]] {
        c.label(pin, NetLabel::global("GND"));
    }
    c.label(u["1"], NetLabel::local("OUT_A"));
    c.label(u["7"], NetLabel::local("OUT_B"));
    let commands = [
        "op".to_string(),
        r#"let a = v("/OUT_A")"#.to_string(),
        r#"let b = v("/OUT_B")"#.to_string(),
        "print a b".to_string(),
    ];
    let out = run(c, &[".options rshunt=1e12"], &commands, &["a", "b"]);
    assert!((out[0] - 1.0).abs() < 0.01 && (out[1] - 2.0).abs() < 0.01, "{out:?}");
}
