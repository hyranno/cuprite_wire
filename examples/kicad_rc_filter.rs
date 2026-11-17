//! Writes the KiCad netlist of a two-stage RC low-pass filter to stdout.
//!
//! Run with `cargo run --example kicad_rc_filter > rc_filter.net`.

mod common;

use std::io;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (circuit, _, _) = common::two_stage_filter();
    let netlist = circuit
        .build(&())
        .to_netlist()?
        .to_kicad()?
        .with_source("examples/kicad_rc_filter.rs");
    netlist.write(&mut io::stdout().lock())?;
    Ok(())
}
