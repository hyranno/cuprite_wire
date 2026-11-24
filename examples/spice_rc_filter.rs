//! Simulates the two-stage RC low-pass filter of `kicad_rc_filter` with ngspice.
//!
//! The filter is put in a test bench with a simulation-only voltage source, and the
//! -3 dB frequency is measured with an AC analysis.
//!
//! Run with `cargo run --example spice_rc_filter` (prints the deck and the result),
//! or `cargo run --example spice_rc_filter -- --deck > rc_filter.cir` to only write the deck.

mod common;

use cuprite_wire::{
    circuit::Named,
    naming::NetLabel,
    spice::{IndependentSource, analysis::Ac},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut bench, input, _) = common::two_stage_filter();
    let source = bench.instantiate_part(Named(
        "source",
        IndependentSource::voltage(0.0).with_ac(1.0),
    ));
    bench.connect(source[IndependentSource::P], input);
    bench.label(source[IndependentSource::N], NetLabel::global("GND"));

    let netlist = bench
        .build(&())
        .to_netlist()?
        .to_spice()?
        .with_title("spice_rc_filter");
    let deck = netlist.deck();
    // The output is labeled `OUT` inside the second stage.
    let out = deck.voltage("/stage2/OUT")?;
    let ac = Ac::decade(20, 1.0, 100e3).probe(&out);
    let deck = deck.analysis(&ac);
    print!("{deck}");
    if std::env::args().any(|arg| arg == "--deck") {
        return Ok(());
    }

    let result = deck.run()?.result(&ac)?;
    let gain: Vec<f64> = result.get(&out)?.iter().map(|v| v.db()).collect();
    let f3db = crossing(result.frequency(), &gain, -3.0).ok_or("the gain stays above -3 dB")?;
    println!("* -3 dB frequency: {f3db:.1} Hz");
    Ok(())
}

/// The first frequency where `gain` falls to `level`, interpolated linearly on the log scale.
fn crossing(frequency: &[f64], gain: &[f64], level: f64) -> Option<f64> {
    let i = gain
        .windows(2)
        .position(|g| g[0] > level && g[1] <= level)?;
    let ratio = (level - gain[i]) / (gain[i + 1] - gain[i]);
    let (low, high) = (frequency[i].ln(), frequency[i + 1].ln());
    Some((low + ratio * (high - low)).exp())
}
