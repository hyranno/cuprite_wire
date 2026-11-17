//! Simulates the two-stage RC low-pass filter of `kicad_rc_filter` with ngspice.
//!
//! The filter is put in a test bench with a simulation-only voltage source, and the
//! -3 dB frequency is measured with an AC analysis.
//!
//! Run with `cargo run --example spice_rc_filter` (prints the deck and the result),
//! or `cargo run --example spice_rc_filter -- --deck > rc_filter.cir` to only write the deck.

mod common;

use cuprite_wire::{naming::NetLabel, spice::IndependentSource};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut bench, input, _) = common::two_stage_filter();
    let source =
        bench.instantiate_part_named("source", IndependentSource::voltage(0.0).with_ac(1.0));
    bench.connect(source["p"], input);
    bench.label(source["n"], NetLabel::global("GND"));

    let netlist = bench
        .build(&())
        .to_netlist()?
        .to_spice()?
        .with_title("spice_rc_filter");
    // The output is labeled `OUT` inside the second stage.
    let out = netlist
        .node("/stage2/OUT")
        .expect("the output is used by the filter")
        .to_string();
    let deck = netlist.deck().control([
        "ac dec 20 1 100k".to_string(),
        format!(r#"meas ac f3db WHEN vdb("{out}")=-3"#),
    ]);
    print!("{deck}");
    if std::env::args().any(|arg| arg == "--deck") {
        return Ok(());
    }

    let simulation = deck.run()?;
    let f3db = simulation.value("f3db").ok_or("f3db was not measured")?;
    println!("* -3 dB frequency: {f3db:.1} Hz");
    Ok(())
}
