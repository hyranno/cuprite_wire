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
//! near its rails. Model types that are not supported (ECL, differential and
//! series models) are listed, not failed.
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
    ibis::{Corner, IbisFile, Model, PinModel, TypMinMax},
    naming::NetLabel,
    spice::{
        digital::{DigitalLoad, DigitalSequence, InputCharacteristics, Level, Pull},
        ibis::{IbisBuffer, IbisBufferError, IbisDriver, IbisReceiver},
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

/// The voltages (low, high) the model drives between, at the typical corner.
fn rails(model: &Model) -> (f64, f64) {
    let typ = |v: Option<TypMinMax<f64>>| v.map(|v| v.typ);
    let low = typ(model.pulldown_reference).unwrap_or(0.0);
    let high = typ(model.pullup_reference)
        .or(typ(model.voltage_range))
        .or(typ(model.power_clamp_reference))
        .unwrap_or(3.3);
    (low, high)
}

/// The buffer (its pin to the line) with 1 kohm to `pull`, simulated until `STOP`.
fn simulate(buffer: IbisBuffer, sequence: Option<DigitalSequence>, pull: f64) -> Table {
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
    let load = circuit.instantiate_part(DigitalLoad::new(
        InputCharacteristics::cmos(3.3)
            .with_capacitance(0.0)
            .with_pull(Pull::up(pull, 1e3)),
    ));
    circuit.connect(pin, load["in"]);
    circuit.label(pin, NetLabel::local("LINE"));
    circuit.label(gnd, NetLabel::global("GND"));
    circuit.label(load["gnd"], NetLabel::global("GND"));
    let deck = deck_of(&circuit).control(vec![
        format!("tran 50p {STOP:e}"),
        format!("wrdata line.txt {LINE}"),
    ]);
    Ngspice::new("ngspice")
        .run(&deck)
        .unwrap_or_else(|e| panic!("{e}"))
        .table("line.txt")
        .unwrap()
}

fn check(name: &str, model: &Model, buffer: IbisBuffer) {
    let (low, high) = rails(model);
    let mid = (low + high) / 2.0;
    let swing = high - low;
    let what = format!("{name}: model `{}` ({})", model.name, model.model_type);
    if !buffer.is_driver() {
        let table = simulate(buffer, None, mid);
        let v = table.interpolate(LINE, STOP).unwrap();
        // Receivers may have pull-ups and pull-downs in their clamps, and terminators pull
        // the line anywhere between their references.
        assert!(
            v > low - 0.1 * swing && v < high + 0.1 * swing,
            "{what}: {v} V"
        );
        return;
    }
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
    let sequence = DigitalSequence::new(first).at(HIGH, second).at(LOW, first);
    let table = simulate(buffer, Some(sequence), mid);
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
