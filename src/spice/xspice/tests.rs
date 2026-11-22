//! Tests with models in C, built with `gcc` against the `cosim.h` of ngspice.

use std::{
    path::{Component, Path, PathBuf},
    process::Command,
};

use super::{DCosim, Verilator, VerilatorError};
use crate::{
    circuit::{CircuitBuilder, Named},
    naming::NetLabel,
    parts::Part,
    spice::{
        IndependentSource, SpicePartInfo,
        digital::{
            DigitalDriver, DigitalLoad, DigitalSequence, InputCharacteristics, Level,
            OutputCharacteristics,
        },
        temp_dir::TempDir,
    },
    test_parts::deck_of,
};

/// A new empty temporary directory for a test, removed when dropped.
fn test_dir(name: &str) -> TempDir {
    TempDir::new(&format!("xspice-{name}")).unwrap()
}

/// An absolute library path, for the tests not loading it.
const LIBRARY: &str = if cfg!(windows) {
    "C:/lib/libx.so"
} else {
    "/lib/libx.so"
};

/// Builds `tests/fixtures/xspice/<name>.c` into a shared library in a new temporary directory.
/// Returns the directory, to be kept while the library is used, and the library.
fn build_library(name: &str) -> (TempDir, PathBuf) {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/xspice");
    let dir = test_dir(name);
    let library = dir.join(format!("lib{name}.so"));
    let status = Command::new("gcc")
        .args(["-shared", "-fPIC", "-I/usr/share/ngspice/scripts/src", "-o"])
        .arg(&library)
        .arg(fixtures.join(format!("{name}.c")))
        .status()
        .expect("gcc runs");
    assert!(status.success(), "gcc failed for {name}.c");
    (dir, library)
}

#[test]
fn test_subcircuit() {
    let part = DCosim::new(LIBRARY)
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
             EI_clk clk#ai 0 clk ref 1\n\
             AI_clk [clk#ai] [clk#di] adc_clk\n\
             .model adc_clk adc_bridge( in_low=1 in_high=2 )\n\
             AC [clk#di] [q#do] cosim\n\
             .model cosim d_cosim( simulation=\"{LIBRARY}\" sim_args=[\"design.vvp\"] delay=2e-9 )\n\
             AO_q [q#do] [q#ao] dac_q\n\
             .model dac_q dac_bridge( out_low=0 out_high=3.3 out_undef=1.65 t_rise=2e-9 t_fall=2e-9 )\n\
             ED_q q#d ref q#ao 0 1\n\
             RO_q q#d q 50\n\
             .ends {name}\n"
        )
    );
    let no_inputs = DCosim::new(LIBRARY).output("q", OutputCharacteristics::cmos(3.3));
    assert!(no_inputs.subcircuit().1.contains("AC NULL [q#do] cosim\n"));
    assert!(matches!(part.spice(), Some(SpicePartInfo::Elements { .. })));
}

/// The internal nodes of a pin are not the node of another pin (`ai_a` was both the internal node
/// of the input `a` and the node of the output `ai_a`).
#[test]
fn test_internal_nodes_apart_from_pins() {
    let part = DCosim::new(LIBRARY)
        .input("a", InputCharacteristics::cmos(3.3))
        .output("ai_a", OutputCharacteristics::cmos(3.3));
    let (_, text) = part.subcircuit();
    assert!(text.contains("EI_a a#ai 0 a ref 1\n"), "{text}");
    assert!(text.contains("RO_ai_a ai_a#d ai_a 50\n"), "{text}");
}

#[test]
#[should_panic(expected = "the pin name \"REF\" is reserved")]
fn test_pin_named_ref() {
    let _ = DCosim::new(LIBRARY).input("REF", InputCharacteristics::cmos(3.3));
}

#[test]
#[should_panic(expected = "is not absolute")]
fn test_relative_library() {
    let _ = DCosim::new("libx.so");
}

#[test]
#[should_panic(expected = "cannot be written in a model")]
fn test_quote_in_argument() {
    let _ = DCosim::new(LIBRARY).with_sim_args(["a\"b"]);
}

/// The C inverter between a driver and a load, with the device's `gnd` 1 V above the ground:
/// the output follows the inverted input, relative to `gnd`.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "builds code models with gcc, supported on Linux only"
)]
fn test_simulate_inverter() {
    let (_dir, library) = build_library("inverter");
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
    let deck = deck_of(&bench).control(["tran 10n 6u", r#"wrdata y.txt v("/Y","/VSS")"#]);
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
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "builds code models with gcc, supported on Linux only"
)]
fn test_simulate_oscillator() {
    let (_dir, library) = build_library("oscillator");
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
    let deck = deck_of(&bench).control(["tran 10n 9u", r#"wrdata q.txt v("/Q")"#]);
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
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "Verilator builds are supported on Linux only"
)]
fn test_simulate_verilog_counter() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/xspice/counter.v");
    let dir = test_dir("counter");
    let build = || {
        Verilator::new([&source])
            .cache_dir(dir.join("cache"))
            .build()
            .unwrap()
    };
    let model = build();
    assert_eq!(build(), model);
    let input = InputCharacteristics::cmos(3.3);
    let cmos = OutputCharacteristics::cmos(3.3);
    let part = model.part(&input, &cmos);
    assert_eq!(part.pin_ids(), ["clk", "rst", "q_1", "q_0", "carry", "gnd"]);

    let mut bench = CircuitBuilder::new();
    let counter = bench.instantiate_part(Named("counter", part));
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
    let deck = deck_of(&bench).control([
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

/// The entries of a cache directory that are builds in progress (or kept after a failure).
fn staging_dirs(cache: &Path) -> Vec<String> {
    std::fs::read_dir(cache)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("build-"))
        .collect()
}

/// A design Verilator rejects is reported with its log, and its build directory is kept.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "Verilator builds are supported on Linux only"
)]
fn test_verilator_error() {
    let dir = test_dir("broken");
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
        matches!(&error, VerilatorError::Command { log, dir, .. }
            if log.contains("broken.v:2") && dir.as_ref().is_some_and(|dir| dir.is_dir())),
        "{error}"
    );
}

/// A design whose ports cannot be pins is reported, and its build directory is removed.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "Verilator builds are supported on Linux only"
)]
fn test_verilator_port_error() {
    let dir = test_dir("gnd-port");
    let source = dir.join("buffer.v");
    std::fs::write(
        &source,
        "module buffer(input gnd, input a, output y);\n  assign y = a;\nendmodule\n",
    )
    .unwrap();
    let cache = dir.join("cache");
    let error = Verilator::new([&source])
        .cache_dir(&cache)
        .build()
        .unwrap_err();
    assert!(
        matches!(&error, VerilatorError::Ports(reason) if reason.contains("\"gnd\"")),
        "{error}"
    );
    assert_eq!(staging_dirs(&cache), Vec::<String>::new());
}

/// A Verilog buffer in `dir`.
fn write_buffer(dir: &Path) -> PathBuf {
    let source = dir.join("buffer.v");
    std::fs::write(
        &source,
        "module buffer(input a, output y);\n  assign y = a;\nendmodule\n",
    )
    .unwrap();
    source
}

/// A missing shim is reported with the setting to change, before Verilator is run (outside Linux
/// with the note that the build is supported on Linux only).
#[test]
fn test_verilator_missing_shim() {
    let dir = test_dir("missing-shim");
    let source = write_buffer(&dir);
    let error = Verilator::new([&source])
        .program(dir.join("no-such-verilator"))
        .shim_dir(dir.join("no-such-shim"))
        .cache_dir(dir.join("cache"))
        .build()
        .unwrap_err();
    let message = error.to_string();
    assert!(
        matches!(&error, VerilatorError::Io(e) if e.kind() == std::io::ErrorKind::NotFound),
        "{message}"
    );
    assert!(
        message.contains("verilator_shim.cpp") && message.contains("`shim_dir`"),
        "{message}"
    );
    assert_eq!(
        message.contains("Linux only"),
        cfg!(not(target_os = "linux")),
        "{message}"
    );
}

/// A missing Verilator is reported with its name (outside Linux with the note that the build is
/// supported on Linux only).
#[test]
fn test_verilator_missing_program() {
    let dir = test_dir("missing-verilator");
    let source = write_buffer(&dir);
    let shim = dir.join("shim");
    std::fs::create_dir(&shim).unwrap();
    std::fs::write(shim.join("verilator_shim.cpp"), "").unwrap();
    std::fs::write(shim.join("verilator_main.cpp"), "").unwrap();
    let error = Verilator::new([&source])
        .program(dir.join("no-such-verilator"))
        .shim_dir(&shim)
        .cache_dir(dir.join("cache"))
        .build()
        .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("cannot run") && message.contains("no-such-verilator"),
        "{message}"
    );
    assert_eq!(
        message.contains("Linux only"),
        cfg!(not(target_os = "linux")),
        "{message}"
    );
}

/// A source that is not UTF-8 (a Latin-1 comment) can be built.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "Verilator builds are supported on Linux only"
)]
fn test_verilator_latin1_source() {
    let dir = test_dir("latin1");
    let source = dir.join("buffer.v");
    let mut text = b"// caf\xe9\n".to_vec();
    text.extend_from_slice(b"module buffer(input a, output y);\n  assign y = a;\nendmodule\n");
    std::fs::write(&source, text).unwrap();
    Verilator::new([&source])
        .cache_dir(dir.join("cache"))
        .build()
        .unwrap();
}

/// Relative paths are resolved against the current directory: the sources, the cache directory
/// (the library is absolute) and the include directories in the arguments. A change to an
/// included file is a new build.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "Verilator builds are supported on Linux only"
)]
fn test_verilator_relative_paths_and_includes() {
    let dir = test_dir("include");
    std::fs::create_dir(dir.join("inc")).unwrap();
    std::fs::write(
        dir.join("top.v"),
        "`include \"width.vh\"\n\
         module top(input clk, output reg [W-1:0] q);\n\
         always @(posedge clk) q <= q + 1;\n\
         endmodule\n",
    )
    .unwrap();
    std::fs::write(dir.join("inc/width.vh"), "localparam W = 2;\n").unwrap();
    // The test directory relative to the current directory.
    let cwd = std::env::current_dir().unwrap();
    let relative: PathBuf = cwd
        .components()
        .skip(1)
        .map(|_| Component::ParentDir)
        .chain(dir.components().skip(1))
        .collect();
    let build = || {
        Verilator::new([relative.join("top.v")])
            .arg(format!("-I{}", relative.join("inc").display()))
            .cache_dir(relative.join("cache"))
            .build()
            .unwrap()
    };
    let model = build();
    assert!(model.library().is_absolute(), "{model:?}");
    assert_eq!(model.outputs()[0].width(), 2);
    assert_eq!(build(), model);

    std::fs::write(dir.join("inc/width.vh"), "localparam W = 3 ;\n").unwrap();
    let changed = build();
    assert_eq!(changed.library(), model.library());
    assert_eq!(changed.outputs()[0].width(), 3);
    assert_eq!(staging_dirs(&dir.join("cache")), Vec::<String>::new());
}
