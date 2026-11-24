//! A single-supply analog front end for a sensor, in front of an ADC: a board described with
//! subcircuits, each tested alone with ERC and SPICE, and exported to pcbnew.
//!
//! ```text
//! J1 IN --> [preamp: AC coupling, x11] --> [filter: 2nd-order low-pass, 1.1 kHz] --> J1 OUT
//!                    ^                               ^
//!                    +----------- [bias: VREF = +5V / 2]
//! ```
//!
//! Run `cargo run --example sensor_frontend > sensor_frontend.net` to check the board with ERC
//! and write its KiCad netlist, `cargo run --example sensor_frontend -- --bom > bom.csv` to write
//! its bill of materials instead, and `cargo test --example sensor_frontend` to test the
//! subcircuits. See `examples/sensor_frontend/README.md`.

mod bias;
mod filter;
mod parts;
mod preamp;

#[cfg(test)]
mod bench;

use std::{io, process::ExitCode};

use cuprite_wire::{
    circuit::{CircuitBuilder, Named},
    erc::ErcSettings,
    kicad::BomOptions,
    naming::NetLabel,
};

use crate::{bias::MidSupply, filter::LowPass, parts::Header1x04, preamp::Preamp};

/// The whole board.
fn board() -> CircuitBuilder {
    let mut board = CircuitBuilder::new();

    // J1: 1 = +5V, 2 = GND, 3 = sensor signal, 4 = filtered output to the ADC.
    let j1 = board.instantiate_part(Named("j1", Header1x04::new()));
    let vcc = j1["1"];
    let gnd = j1["2"];
    board.label(vcc, NetLabel::global("+5V"));
    board.label(gnd, NetLabel::global("GND"));
    board.label(j1["3"], NetLabel::local("IN"));
    board.label(j1["4"], NetLabel::local("OUT"));
    // The supply comes in through the connector's passive pins: flag it for ERC, as with
    // KiCad's PWR_FLAG.
    board.power_flag(vcc);
    board.power_flag(gnd);

    let bias = board.instantiate_subcircuit(Named("bias", MidSupply));
    let preamp = board.instantiate_subcircuit(Named("preamp", Preamp::default()));
    let filter = board.instantiate_subcircuit(Named("filter", LowPass));
    for (stage_vcc, stage_gnd) in [
        (bias.vcc, bias.gnd),
        (preamp.vcc, preamp.gnd),
        (filter.vcc, filter.gnd),
    ] {
        board.connect(stage_vcc, vcc);
        board.connect(stage_gnd, gnd);
    }
    board.label(bias.vref, NetLabel::local("VREF"));
    board.connect(preamp.vref, bias.vref);
    board.connect(filter.vref, bias.vref);

    board.connect(j1["3"], preamp.input);
    board.connect(preamp.output, filter.input);
    board.label(preamp.output, NetLabel::local("AMP"));
    board.connect(filter.output, j1["4"]);
    board
}

fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let netlist = board().build(&()).to_netlist()?;
    let report = netlist.erc(&ErcSettings::default());
    if !report.is_clean() {
        eprintln!("{report}");
    }
    if report.has_errors() {
        eprintln!("ERC failed; the netlist is not written");
        return Ok(ExitCode::FAILURE);
    }
    let kicad = netlist
        .to_kicad()?
        .with_source("examples/sensor_frontend/main.rs");
    if std::env::args().any(|arg| arg == "--bom") {
        kicad
            .bom(&BomOptions::default())
            .write(&mut io::stdout().lock())?;
    } else {
        kicad.write(&mut io::stdout().lock())?;
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The board as a whole: the subcircuits are connected consistently.
    #[test]
    fn board_erc() {
        board()
            .build(&())
            .to_netlist()
            .unwrap()
            .erc(&ErcSettings::default())
            .assert_clean();
    }

    /// The board exports to KiCad: every part has a footprint with its pads.
    #[test]
    fn board_netlist() {
        let netlist = board().build(&()).to_netlist().unwrap();
        let mut out = Vec::new();
        netlist.to_kicad().unwrap().write(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(r#"(footprint "sensor_frontend:PinHeader_1x04_P2.54mm_Vertical")"#));
        assert!(text.contains(r#"(footprint "cuprite_basic:SOT-23-5")"#));
    }

    /// The bill of materials groups the parts by value and footprint.
    #[test]
    fn board_bom() {
        let netlist = board().build(&()).to_netlist().unwrap();
        let bom = netlist.to_kicad().unwrap().bom(&BomOptions::default());
        let text = bom.to_string();
        assert!(text.starts_with("\"Refs\",\"Value\",\"Footprint\",\"Qty\",\"DNP\"\n"));
        assert!(text.contains("\"U1-U3\",\"OPAMP_RRIO\",\"cuprite_basic:SOT-23-5\",\"3\",\"\"\n"));
    }
}
