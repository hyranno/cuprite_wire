//! Parts made from real vendor models, which cannot be committed (their licenses restrict
//! redistribution). Opt-in: download the models, put them into one directory and run
//!
//! ```sh
//! CUPRITE_VENDOR_MODELS_DIR=/path/to/models cargo test --features vendor-models --test vendor_models
//! ```
//!
//! Expected files (rename them, or adjust the paths below):
//! - `TPS3899DL01.lib`: TI's PSpice model of the TPS3899DL01 voltage supervisor, with the
//!   `.subckt` `TPS3899DL01` and the ports `CTR CTS GND RESET_B SENSE VDD` (skidl discussion #293).
//! - `LM358.lib`: TI's PSpice model of one LM358 amplifier, with the ports `IN+ IN- VCC VEE OUT`.
//!
//! The names are assumptions (not checked against the files, see `doc/spice_part_macro.md`):
//! if a name differs, the compile error lists the names and ports the library has.
//! The tests check the behavior, not golden files. The LM358 part also needs the KiCad libraries.

use cuprite_wire::{
    basic::Resistor0603, circuit::CircuitBuilder, kicad_part, naming::NetLabel, spice::IndependentSource,
    spice_part,
};

spice_part! {
    /// TPS3899DL01 with TI's model; the reset output is renamed to the datasheet's name.
    struct Tps3899 {
        library: "${CUPRITE_VENDOR_MODELS_DIR}/TPS3899DL01.lib",
        name: "TPS3899DL01",
        pins: { "RESET_B" => "reset_n" },
    }
}

kicad_part! {
    /// KiCad's LM358 symbol with TI's single-amplifier model used for both units.
    struct Lm358 {
        symbol: "Amplifier_Operational:LM358",
        footprint: none,
        sim: [
            { library: "${CUPRITE_VENDOR_MODELS_DIR}/LM358.lib", name: "LM358", suffix: "A",
              pins: { "3" => "IN+", "2" => "IN-", "8" => "VCC", "4" => "VEE", "1" => "OUT" } },
            { library: "${CUPRITE_VENDOR_MODELS_DIR}/LM358.lib", name: "LM358", suffix: "B",
              pins: { "5" => "IN+", "6" => "IN-", "8" => "VCC", "4" => "VEE", "7" => "OUT" } },
        ],
    }
}

fn run(circuit: CircuitBuilder, probes: &[&str]) -> Vec<f64> {
    let mut control = vec!["op".to_string()];
    for (i, probe) in probes.iter().enumerate() {
        control.push(format!("let p{i} = v(\"{probe}\")"));
        control.push(format!("print p{i}"));
    }
    let deck = circuit.build(&()).to_netlist().unwrap().to_spice().unwrap().deck().control(control);
    let sim = deck.run().unwrap_or_else(|e| panic!("{e}"));
    (0..probes.len())
        .map(|i| sim.value(&format!("p{i}")).unwrap_or_else(|| panic!("{}", sim.log())))
        .collect()
}

/// RESET (pulled up to VDD) is low with SENSE at 0 V and high with SENSE at VDD.
#[test]
fn test_tps3899() {
    let reset = |sense: f64| {
        let mut c = CircuitBuilder::new();
        let u = c.instantiate_part(Box::new(Tps3899::new()));
        let supply = c.instantiate_part(IndependentSource::voltage(3.3));
        let input = c.instantiate_part(IndependentSource::voltage(sense));
        // The output may be open-drain.
        let pullup = c.instantiate_part(Box::new(Resistor0603::new("10k")));
        c.connect(pullup["1"], u["VDD"]);
        c.connect(pullup["2"], u[Tps3899::RESET_N]);
        c.connect(supply["p"], u["VDD"]);
        c.connect(input["p"], u["SENSE"]);
        for pin in [supply["n"], input["n"], u["GND"], u["CTR"], u["CTS"]] {
            c.label(pin, NetLabel::global("GND"));
        }
        c.label(u["VDD"], NetLabel::global("VDD"));
        c.label(u[Tps3899::RESET_N], NetLabel::local("RESET"));
        run(c, &["/RESET"])[0]
    };
    assert!(reset(0.0) < 0.3, "RESET with SENSE at 0 V");
    assert!(reset(3.3) > 3.0, "RESET with SENSE at VDD");
}

/// Both units of the LM358 work as followers.
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
    let out = run(c, &["/OUT_A", "/OUT_B"]);
    assert!((out[0] - 1.0).abs() < 0.05 && (out[1] - 2.0).abs() < 0.05, "{out:?}");
}
