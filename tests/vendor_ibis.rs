//! IBIS models of real devices, which cannot be committed (their licenses restrict
//! redistribution, or are unknown). Opt-in: put `.ibs` files into `<dir>/ibis/` and run
//!
//! ```sh
//! CUPRITE_VENDOR_MODELS_DIR=/path/to/models cargo test --features vendor-models,ibis --test vendor_ibis
//! ```
//!
//! Every file of the directory must parse. Every buffer of every pin (each model of a
//! `[Model Selector]`) is built at the three corners, and simulated at the typical one: a driver
//! plays low, high (or released) and low into 1 kohm to half its supply, and the line must be
//! higher in the middle (and at half the supply where released); a receiver must keep the line
//! near its rails. ECL models are pulled to their supply minus 2 V through 50 ohms instead.
//! Every pair of `[Diff Pin]` is built the same way (with its `[Series Pin Mapping]`
//! termination), and simulated with the pull on both pins: a driver must swap the sign of the
//! voltage between its pins. Model types that are not supported (series switch and `*_diff`
//! models) are listed, not failed.
//!
//! The files used during development (2026-11-22), all from public repositories on GitHub: the
//! samples of pybis (russdill/pybis: `sample2`, `diff_pecl_term`, `cbt`, `bird57ex`, `sterm`,
//! `bushold`), Xyce's regression `at16245.ibs`, and vendor files: TI `ahc1g125` and
//! `sn74hc_5v`, Micrel `89832u`, Micron `r_120`, ADI `adv7511` and `ad9744aru`, Microchip
//! `ksz9567s`, FTDI `ft232r_vccio5`, Lattice `ice40_lvcmos33`, and the files of two FPGA/CPLD
//! designs (`mux.ibs`, `modexp.ibs`). KiCad's QA files parse but do not belong here: their
//! values are made up (millihenries of package inductance, clamps conducting inside the rails).

mod common;

use common::deck_of;
use cuprite_wire::{
    circuit::CircuitBuilder,
    ibis::{Corner, IbisFile, Model, ModelType, PinModel, TypMinMax},
    naming::NetLabel,
    spice::{
        digital::{DigitalLoad, DigitalSequence, InputCharacteristics, Level, Pull},
        ibis::{
            IbisBuffer, IbisBufferError, IbisDiffBuffer, IbisDiffDriver, IbisDiffReceiver,
            IbisDriver, IbisReceiver,
        },
        simulation::{Ngspice, Table},
    },
};

const LINE: &str = r#"v("/LINE")"#;

/// The phases of the driver test, in seconds: low until `HIGH`, high (or released) until `LOW`,
/// low until `STOP`.
const HIGH: f64 = 50e-9;
const LOW: f64 = 150e-9;
const STOP: f64 = 250e-9;

fn files() -> Vec<(String, IbisFile)> {
    let dir =
        std::path::Path::new(&std::env::var("CUPRITE_VENDOR_MODELS_DIR").unwrap()).join("ibis");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ibs")))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no .ibs files in {}", dir.display());
    paths
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            // Some files are Latin-1 in their comments.
            let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
            let ibs = cuprite_wire::ibis::parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
            (name, ibs)
        })
        .collect()
}

/// The voltages (low, high) the model drives between, at the typical corner, and the resistance
/// of the pull of the test to their middle.
fn rails(model: &Model) -> (f64, f64, f64) {
    let typ = |v: Option<TypMinMax<f64>>| v.map(|v| v.typ);
    let high = typ(model.pullup_reference)
        .or(typ(model.voltage_range))
        .or(typ(model.power_clamp_reference))
        .unwrap_or(3.3);
    if matches!(
        model.model_type,
        ModelType::InputEcl | ModelType::OutputEcl | ModelType::IoEcl | ModelType::ThreeStateEcl
    ) {
        // 50 ohms to the supply minus 2 V, between the levels of about 0.9 V and 1.7 V below
        // the supply.
        return (high - 2.4, high - 1.6, 50.0);
    }
    let low = typ(model.pulldown_reference).unwrap_or(0.0);
    (low, high, 1e3)
}

/// The test bench of [`simulate`] and [`simulate_diff`]: the pull on each of the `lines` (local
/// labels), and the simulation until `STOP`, writing the voltages of the lines.
fn run(
    mut circuit: CircuitBuilder,
    lines: &[(cuprite_wire::circuit::Node, &str)],
    pull: (f64, f64),
) -> Table {
    let mut columns = Vec::new();
    for &(pin, label) in lines {
        let load = circuit.instantiate_part(DigitalLoad::new(
            InputCharacteristics::cmos(3.3)
                .with_capacitance(0.0)
                .with_pull(Pull::up(pull.0, pull.1)),
        ));
        circuit.connect(pin, load["in"]);
        circuit.label(pin, NetLabel::local(label));
        circuit.label(load["gnd"], NetLabel::global("GND"));
        columns.push(format!(r#"v("/{label}")"#));
    }
    let deck = deck_of(&circuit).control(vec![
        format!("tran 50p {STOP:e}"),
        format!("wrdata line.txt {}", columns.join(" ")),
    ]);
    Ngspice::new("ngspice")
        .run(&deck)
        .unwrap_or_else(|e| panic!("{e}"))
        .table("line.txt")
        .unwrap()
}

/// The pair with the pull `(voltage, resistance)` on both pins (`/P` and `/N`).
fn simulate_diff(
    buffer: IbisDiffBuffer,
    sequence: Option<DigitalSequence>,
    pull: (f64, f64),
) -> Table {
    let mut circuit = CircuitBuilder::new();
    let (p, n, gnd) = match sequence {
        Some(sequence) => {
            let d = circuit.instantiate_part(IbisDiffDriver::new(buffer, sequence));
            (
                d[IbisDiffDriver::P],
                d[IbisDiffDriver::N],
                d[IbisDiffDriver::GND],
            )
        }
        None => {
            let r = circuit.instantiate_part(IbisDiffReceiver::new(buffer));
            (
                r[IbisDiffReceiver::P],
                r[IbisDiffReceiver::N],
                r[IbisDiffReceiver::GND],
            )
        }
    };
    circuit.label(gnd, NetLabel::global("GND"));
    run(circuit, &[(p, "P"), (n, "N")], pull)
}

/// The buffer (its pin to the line) with the pull `(voltage, resistance)`, simulated until
/// `STOP`.
fn simulate(buffer: IbisBuffer, sequence: Option<DigitalSequence>, pull: (f64, f64)) -> Table {
    let mut circuit = CircuitBuilder::new();
    let (pin, gnd) = match sequence {
        Some(sequence) => {
            let d = circuit.instantiate_part(IbisDriver::new(buffer, sequence));
            (d[IbisDriver::OUT], d[IbisDriver::GND])
        }
        None => {
            let r = circuit.instantiate_part(IbisReceiver::new(buffer));
            (r[IbisReceiver::IN], r[IbisReceiver::GND])
        }
    };
    circuit.label(gnd, NetLabel::global("GND"));
    run(circuit, &[(pin, "LINE")], pull)
}

/// The levels a driver plays: low, then high, then low; released for a level it cannot drive.
fn driver_sequence(buffer: &IbisBuffer) -> DigitalSequence {
    let first = if buffer.can_drive(Level::Low) {
        Level::Low
    } else {
        Level::HiZ
    };
    let second = if buffer.can_drive(Level::High) {
        Level::High
    } else {
        Level::HiZ
    };
    DigitalSequence::new(first).at(HIGH, second).at(LOW, first)
}

fn check(name: &str, model: &Model, buffer: IbisBuffer) {
    let (low, high, r) = rails(model);
    let mid = (low + high) / 2.0;
    let swing = high - low;
    let what = format!("{name}: model `{}` ({})", model.name, model.model_type);
    if !buffer.is_driver() {
        let table = simulate(buffer, None, (mid, r));
        let v = table.interpolate(LINE, STOP).unwrap();
        // Receivers may have pull-ups and pull-downs in their clamps, and terminators pull
        // the line anywhere between their references.
        assert!(
            v > low - 0.1 * swing && v < high + 0.1 * swing,
            "{what}: {v} V"
        );
        return;
    }
    let sequence = driver_sequence(&buffer);
    let [first, second] = [sequence.initial(), sequence.events()[0].1];
    let table = simulate(buffer, Some(sequence), (mid, r));
    let at = |t: f64| table.interpolate(LINE, t).unwrap();
    let levels = [(HIGH, first), (LOW, second), (STOP, first)].map(|(t, level)| (t, level, at(t)));
    // A released buffer leaves the line to the pull.
    for (time, level, v) in levels {
        if level == Level::HiZ {
            assert!(
                (v - mid).abs() < 0.1 * swing,
                "{what}: {v} V at {time:e} s, not released"
            );
        }
    }
    // The levels need not be near the rails (TTL or LVDS outputs are not), but must differ.
    let [(_, _, v1), (_, _, v2), (_, _, v3)] = levels;
    assert!(
        v2 - v1.max(v3) > 0.1 * swing,
        "{what}: {v1} V, {v2} V, {v3} V"
    );
}

fn check_diff(name: &str, model: &Model, pair: IbisDiffBuffer) {
    let (low, high, r) = rails(model);
    let mid = (low + high) / 2.0;
    let swing = high - low;
    let what = format!(
        "{name}: diff pair of model `{}` ({})",
        model.name, model.model_type
    );
    // Open pins release where they cannot drive.
    let (driver, sequence) = if pair.p().is_driver() {
        let sequence = DigitalSequence::new(Level::Low)
            .at(HIGH, Level::High)
            .at(LOW, Level::Low);
        (true, Some(sequence))
    } else {
        (false, None)
    };
    let table = simulate_diff(pair, sequence, (mid, r));
    let v = |pin: &str, t: f64| table.interpolate(&format!(r#"v("/{pin}")"#), t).unwrap();
    let diff = |t: f64| v("P", t) - v("N", t);
    if !driver {
        for pin in ["P", "N"] {
            let v = v(pin, STOP);
            assert!(
                v > low - 0.1 * swing && v < high + 0.1 * swing,
                "{what}: {pin} at {v} V"
            );
        }
        return;
    }
    let [d1, d2, d3] = [HIGH, LOW, STOP].map(diff);
    assert!(
        d2 - d1.max(d3) > 0.1 * swing,
        "{what}: {d1} V, {d2} V, {d3} V between the pins"
    );
}

#[test]
fn vendor_ibis_diff_pins() {
    let mut unsupported = Vec::new();
    let mut simulated = 0;
    for (name, ibs) in files() {
        for component in &ibs.components {
            for diff in &component.diff_pins {
                let PinModel::Named(pin_model) = &component.pin(&diff.pin).unwrap().model else {
                    panic!("{name}: diff pin {} has no model", diff.pin);
                };
                for corner in [Corner::Typ, Corner::Min, Corner::Max] {
                    let pair =
                        match IbisDiffBuffer::of_pin(&ibs, &component.name, &diff.pin, corner) {
                            Ok(pair) => pair,
                            Err(IbisBufferError::Subcircuit(e))
                                if e.message.ends_with("is not supported") =>
                            {
                                unsupported.push(format!("{name}: {e}"));
                                break;
                            }
                            Err(e) => panic!("{name}: diff pin {}: {e}", diff.pin),
                        };
                    if corner == Corner::Typ {
                        let model = ibs.pin_model(pin_model, None).unwrap();
                        simulated += 1;
                        check_diff(&name, model, pair);
                    }
                }
            }
        }
    }
    assert!(simulated > 0, "no [Diff Pin] in the files");
    println!("simulated {simulated} pairs");
    for entry in unsupported {
        println!("not supported: {entry}");
    }
}

#[test]
fn vendor_ibis_models() {
    let mut unsupported = Vec::new();
    let mut simulated: Vec<IbisBuffer> = Vec::new();
    for (name, ibs) in files() {
        for component in &ibs.components {
            for pin in &component.pins {
                let PinModel::Named(pin_model) = &pin.model else {
                    continue;
                };
                let choices: Vec<String> = match ibs.model_selector(pin_model) {
                    Some(selector) => selector.options.iter().map(|o| o.model.clone()).collect(),
                    None => vec![pin_model.clone()],
                };
                for choice in choices {
                    let model = ibs.pin_model(pin_model, Some(&choice)).unwrap();
                    for corner in [Corner::Typ, Corner::Min, Corner::Max] {
                        let buffer = match IbisBuffer::of_pin_model(
                            &ibs,
                            &component.name,
                            &pin.name,
                            &choice,
                            corner,
                        ) {
                            Ok(buffer) => buffer,
                            Err(IbisBufferError::Subcircuit(e))
                                if e.message.ends_with("is not supported") =>
                            {
                                let entry = format!("{name}: {e}");
                                if !unsupported.contains(&entry) {
                                    unsupported.push(entry);
                                }
                                continue;
                            }
                            Err(e) => panic!("{name}: pin {}: {e}", pin.name),
                        };
                        if corner == Corner::Typ && !simulated.contains(&buffer) {
                            simulated.push(buffer.clone());
                            check(&name, model, buffer);
                        }
                    }
                }
            }
        }
    }
    println!("simulated {} buffers", simulated.len());
    for entry in unsupported {
        println!("not supported: {entry}");
    }
}
