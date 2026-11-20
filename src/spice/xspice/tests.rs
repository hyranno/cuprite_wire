//! Tests with models in C, built with `gcc` against the `cosim.h` of ngspice.

use std::{path::PathBuf, process::Command};

use super::{DCosim, Verilator};
use crate::{
    circuit::CircuitBuilder,
    naming::NetLabel,
    parts::Part,
    spice::{
        IndependentSource, SpicePartInfo,
        digital::{
            DigitalDriver, DigitalLoad, DigitalSequence, InputCharacteristics, Level,
            OutputCharacteristics,
        },
    },
};

/// Builds `tests/fixtures/xspice/<name>.c` into a shared library in a new temporary directory.
fn build_library(name: &str) -> PathBuf {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/xspice");
    let dir = std::env::temp_dir().join(format!(
        "{}-xspice-{}-{name}",
        env!("CARGO_PKG_NAME"),
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let library = dir.join(format!("lib{name}.so"));
    let status = Command::new("gcc")
        .args(["-shared", "-fPIC", "-I/usr/share/ngspice/scripts/src", "-o"])
        .arg(&library)
        .arg(fixtures.join(format!("{name}.c")))
        .status()
        .expect("gcc runs");
    assert!(status.success(), "gcc failed for {name}.c");
    library
}

#[test]
fn test_subcircuit() {
    let part = DCosim::new("/lib/libx.so")
        .with_sim_args(["design.vvp"])
        .with_delay(2e-9)
        .input(
            "clk",
            InputCharacteristics {
                threshold_low: 1.0,
                threshold_high: 2.0,
                ..InputCharacteristics::cmos(3.3).with_capacitance(0.0)
            },
        )
        .output("q", OutputCharacteristics::cmos(3.3));
    assert_eq!(part.pin_ids(), ["clk", "q", "gnd"]);
    let (name, text) = part.subcircuit();
    assert_eq!(
        text,
        format!(
            ".subckt {name} clk q ref\n\
             EI_clk ai_clk 0 clk ref 1\n\
             AI_clk [ai_clk] [di_clk] adc_clk\n\
             .model adc_clk adc_bridge( in_low=1 in_high=2 )\n\
             AC [di_clk] [do_q] cosim\n\
             .model cosim d_cosim( simulation=\"/lib/libx.so\" sim_args=[\"design.vvp\"] delay=2e-9 )\n\
             AO_q [do_q] [ao_q] dac_q\n\
             .model dac_q dac_bridge( out_low=0 out_high=3.3 out_undef=1.65 t_rise=2e-9 t_fall=2e-9 )\n\
             EO_q d_q ref ao_q 0 1\n\
             RO_q d_q q 50\n\
             .ends {name}\n"
        )
    );
    let no_inputs = DCosim::new("/lib/libx.so").output("q", OutputCharacteristics::cmos(3.3));
    assert!(no_inputs.subcircuit().1.contains("AC NULL [do_q] cosim\n"));
    assert!(matches!(part.spice(), Some(SpicePartInfo::Elements { .. })));
}

#[test]
#[should_panic(expected = "is not absolute")]
fn test_relative_library() {
    let _ = DCosim::new("libx.so");
}

#[test]
#[should_panic(expected = "cannot be written in a model")]
fn test_quote_in_argument() {
    let _ = DCosim::new("/lib/libx.so").with_sim_args(["a\"b"]);
}

/// The C inverter between a driver and a load, with the device's `gnd` 1 V above the ground:
/// the output follows the inverted input, relative to `gnd`.
#[test]
fn test_simulate_inverter() {
    let library = build_library("inverter");
    let cmos = OutputCharacteristics::cmos(3.3);
    let input = InputCharacteristics::cmos(3.3);
    let mut bench = CircuitBuilder::new();
    let sent = DigitalSequence::new(Level::Low).pwm(1e-6, 2e-6, 0.5, 2);
    let driver = bench.instantiate_part(DigitalDriver::new(cmos.clone(), sent.clone()));
    let inverter = bench.instantiate_part(
        DCosim::new(&library)
            .input("a", input.clone())
            .output("y", cmos),
    );
    let load = bench.instantiate_part(DigitalLoad::new(input.clone()));
    let offset = bench.instantiate_part(IndependentSource::voltage(1.0));
    bench.connect(driver[DigitalDriver::OUT], inverter["a"]);
    bench.connect(inverter["y"], load[DigitalLoad::IN]);
    bench.label(inverter["y"], NetLabel::local("Y"));
    for gnd in [
        driver[DigitalDriver::GND],
        inverter[DCosim::GND],
        load[DigitalLoad::GND],
        offset[IndependentSource::P],
    ] {
        bench.label(gnd, NetLabel::local("VSS"));
    }
    bench.label(offset[IndependentSource::N], NetLabel::global("GND"));
    let deck = bench
        .build(&())
        .to_netlist()
        .unwrap()
        .to_spice()
        .unwrap()
        .deck()
        .control(["tran 10n 6u", r#"wrdata y.txt v("/Y","/VSS")"#]);
    let sim = deck.run().unwrap();
    let table = sim.table("y.txt").unwrap();
    let y = table.column(r#"v("/Y","/VSS")"#).unwrap();
    assert!((y[y.len() - 1] - 3.3).abs() < 0.01, "{}", y[y.len() - 1]);
    let read = input.decode(table.scale(), y);
    // The model answers its first input, at time 0, after the delays.
    let (first, rest) = read.events().split_first().expect("the output changes");
    assert_eq!(read.initial(), Level::Low, "{read:?}");
    assert!(first.1 == Level::High && first.0 < 10e-9, "{read:?}");
    assert_eq!(rest.len(), sent.events().len(), "{read:?}");
    for (out, edge) in rest.iter().zip(sent.events()) {
        assert_ne!(out.1, edge.1);
        // The delay of the bridges and the model (1 ns each) and the edges.
        assert!(out.0 > edge.0 && out.0 - edge.0 < 10e-9, "{out:?} {edge:?}");
    }
}

/// A model without inputs, configured by its simulation argument: a 250 kHz oscillator.
#[test]
fn test_simulate_oscillator() {
    let library = build_library("oscillator");
    let mut bench = CircuitBuilder::new();
    let oscillator = bench.instantiate_part(
        DCosim::new(&library)
            .with_sim_args(["2e-6"])
            .output("q", OutputCharacteristics::cmos(3.3)),
    );
    let input = InputCharacteristics::cmos(3.3);
    let load = bench.instantiate_part(DigitalLoad::new(input.clone()));
    bench.connect(oscillator["q"], load[DigitalLoad::IN]);
    bench.label(oscillator["q"], NetLabel::local("Q"));
    bench.label(oscillator[DCosim::GND], NetLabel::global("GND"));
    bench.label(load[DigitalLoad::GND], NetLabel::global("GND"));
    let deck = bench
        .build(&())
        .to_netlist()
        .unwrap()
        .to_spice()
        .unwrap()
        .deck()
        .control(["tran 10n 9u", r#"wrdata q.txt v("/Q")"#]);
    let sim = deck.run().unwrap();
    let table = sim.table("q.txt").unwrap();
    let read = input.decode(table.scale(), table.column(r#"v("/Q")"#).unwrap());
    let times: Vec<f64> = read.events().iter().map(|e| e.0).collect();
    assert_eq!(read.initial(), Level::Low, "{read:?}");
    assert_eq!(times.len(), 4, "{read:?}");
    // The model is called at the time points of the analysis (10 ns apart at most).
    for (k, t) in times.iter().enumerate() {
        let expected = 2e-6 * (k + 1) as f64;
        assert!(t - expected > 0.0 && t - expected < 20e-9, "{read:?}");
    }
}

/// A 2-bit counter in Verilog, built with Verilator: clocked every 1 us after a reset, it counts
/// 1, 2, 3, 0 with the carry at 3. The second build comes from the cache.
#[test]
fn test_simulate_verilog_counter() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/xspice/counter.v");
    let model = Verilator::new([&source]).build().unwrap();
    assert_eq!(Verilator::new([&source]).build().unwrap(), model);
    let input = InputCharacteristics::cmos(3.3);
    let cmos = OutputCharacteristics::cmos(3.3);
    let part = model.part(&input, &cmos);
    assert_eq!(part.pin_ids(), ["clk", "rst", "q_1", "q_0", "carry", "gnd"]);

    let mut bench = CircuitBuilder::new();
    let counter = bench.instantiate_part_named("counter", part);
    let clock = bench.instantiate_part(DigitalDriver::new(
        cmos.clone(),
        DigitalSequence::new(Level::Low).pwm(1e-6, 1e-6, 0.5, 5),
    ));
    let reset = bench.instantiate_part(DigitalDriver::new(
        cmos.clone(),
        DigitalSequence::new(Level::High).at(0.5e-6, Level::Low),
    ));
    bench.connect(clock[DigitalDriver::OUT], counter["clk"]);
    bench.connect(reset[DigitalDriver::OUT], counter["rst"]);
    let mut gnds = vec![
        counter[DCosim::GND],
        clock[DigitalDriver::GND],
        reset[DigitalDriver::GND],
    ];
    for (pin, label) in [("q_1", "Q1"), ("q_0", "Q0"), ("carry", "C")] {
        let load = bench.instantiate_part(DigitalLoad::new(input.clone()));
        bench.connect(counter[pin], load[DigitalLoad::IN]);
        bench.label(counter[pin], NetLabel::local(label));
        gnds.push(load[DigitalLoad::GND]);
    }
    for gnd in gnds {
        bench.label(gnd, NetLabel::global("GND"));
    }
    let deck = bench
        .build(&())
        .to_netlist()
        .unwrap()
        .to_spice()
        .unwrap()
        .deck()
        .control([
            "tran 10n 5.5u",
            r#"wrdata out.txt v("/Q1") v("/Q0") v("/C")"#,
        ]);
    let sim = deck.run().unwrap();
    let table = sim.table("out.txt").unwrap();
    let level = |node: &str, time: f64| {
        input
            .decode(
                table.scale(),
                table.column(&format!(r#"v("{node}")"#)).unwrap(),
            )
            .level_at(time)
    };
    use Level::{High as H, Low as L};
    // After the clock edges at 1, 2, 3 and 4 us.
    for (time, q1, q0, carry) in [
        (1.5e-6, L, H, L),
        (2.5e-6, H, L, L),
        (3.5e-6, H, H, H),
        (4.5e-6, L, L, L),
    ] {
        assert_eq!(
            [level("/Q1", time), level("/Q0", time), level("/C", time)],
            [q1, q0, carry],
            "at {time} s"
        );
    }
}

/// A design Verilator rejects is reported with its log.
#[test]
fn test_verilator_error() {
    let dir = std::env::temp_dir().join(format!(
        "{}-xspice-{}-broken",
        env!("CARGO_PKG_NAME"),
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("broken.v");
    std::fs::write(
        &source,
        "module broken(input a, output y);\n  assign y = ;\nendmodule\n",
    )
    .unwrap();
    let error = Verilator::new([&source])
        .cache_dir(dir.join("cache"))
        .build()
        .unwrap_err();
    assert!(
        matches!(&error, super::VerilatorError::Command { log, .. } if log.contains("broken.v:2")),
        "{error}"
    );
}
