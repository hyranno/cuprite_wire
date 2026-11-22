//! XSPICE code models wrapped in a `.subckt`, from the test library
//! `crates/kicad_lib/tests/fixtures/models/xspice/counter2.lib`: a 2-bit counter built from a
//! `d_state` machine whose state file is named by a relative path. Inlined, the path is made
//! absolute, so ngspice finds the file from the directory it runs in.

mod common;

use common::deck_of;
use cuprite_wire::{
    circuit::CircuitBuilder,
    kicad_part,
    naming::NetLabel,
    parts::Part,
    spice::{IndependentSource, Waveform},
    spice_part,
};

spice_part! {
    /// The counter, with its `d_state` model in the subcircuit.
    struct Counter {
        library: "crates/kicad_lib/tests/fixtures/models/xspice/counter2.lib",
        name: "counter2",
        sim_model: inline,
    }
}

spice_part! {
    /// The counter, with its `d_state` model in a file included from another directory.
    struct CounterExt {
        library: "crates/kicad_lib/tests/fixtures/models/xspice/counter2.lib",
        name: "counter2_ext",
        sim_model: inline,
    }
}

kicad_part! {
    /// The counter on a symbol with three pins (the pins mean nothing for a counter).
    struct CounterSymbol {
        symbol: "test:REG",
        sim: {
            library: "models/xspice/counter2.lib",
            name: "counter2",
            pins: { "3" => "clk", "2" => "q0", "1" => "q1" },
        },
        sim_model: inline,
        symbol_dir: "crates/kicad_lib/tests/fixtures/symbols",
        footprint_dir: "crates/kicad_lib/tests/fixtures/footprints",
        sim_library_dir: "crates/kicad_lib/tests/fixtures",
    }
}

/// The outputs `[q0, q1]` (in volts) after each of four clock edges (at 1, 11, 21 and 31 µs).
fn count(part: Box<dyn Part>, [clk, q0, q1]: [&str; 3]) -> Vec<[f64; 2]> {
    let mut circuit = CircuitBuilder::new();
    let u = circuit.instantiate_part(part);
    let clock = circuit.instantiate_part(IndependentSource::voltage(0.0).with_waveform(
        Waveform::Pulse {
            initial: 0.0,
            pulsed: 3.3,
            delay: 1e-6,
            rise: 1e-9,
            fall: 1e-9,
            width: 5e-6,
            period: 10e-6,
        },
    ));
    circuit.connect(clock[IndependentSource::P], u[clk]);
    circuit.label(clock[IndependentSource::N], NetLabel::global("GND"));
    circuit.label(u[q0], NetLabel::local("Q0"));
    circuit.label(u[q1], NetLabel::local("Q1"));
    let times = [8, 18, 28, 38];
    let mut control = vec!["tran 0.1u 40u".to_string()];
    for (i, t) in times.iter().enumerate() {
        control.push(format!("meas tran q0_{i} find v(/Q0) at={t}u"));
        control.push(format!("meas tran q1_{i} find v(/Q1) at={t}u"));
    }
    let sim = deck_of(&circuit)
        .control(control)
        .run()
        .map_err(common::simulation_error_message)
        .unwrap();
    (0..times.len())
        .map(|i| {
            let value = |name: String| {
                sim.value(&name)
                    .unwrap_or_else(|| panic!("no {name}:\n{}", sim.log()))
            };
            [value(format!("q0_{i}")), value(format!("q1_{i}"))]
        })
        .collect()
}

#[test]
fn test_counter() {
    let expected = [[3.3, 0.0], [0.0, 3.3], [3.3, 3.3], [0.0, 0.0]];
    let check = |part: Box<dyn Part>, pins: [&str; 3]| {
        let outputs = count(part, pins);
        for (got, want) in outputs.iter().zip(&expected) {
            assert!(
                got.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.01),
                "{outputs:?}"
            );
        }
    };
    check(Box::new(Counter::new()), ["clk", "q0", "q1"]);
    check(Box::new(CounterExt::new()), ["clk", "q0", "q1"]);
    check(Box::new(CounterSymbol::new()), ["3", "2", "1"]);
}
