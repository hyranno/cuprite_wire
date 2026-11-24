//! Signal integrity of two links between chips, simulated with the IBIS models of their I/O
//! buffers (feature `ibis`).
//!
//! `demo_io.ibs` describes a made-up chip with a 3.3 V LVCMOS output and input and an LVDS-like
//! differential output and input. Two of these chips are linked on a board by traces of 1 ns
//! (about 15 cm):
//!
//! - a 50 MHz clock on a 50 ohm trace, without and with a series resistor at the driver. Without
//!   it, the edge reflects at the receiver and rings; the receiver's clamp diodes cut the
//!   overshoot.
//! - a bit pattern at 500 Mbit/s on a differential pair of 50 ohm traces, terminated by the
//!   receiver's on-die 100 ohms (its `[Series Pin Mapping]`).
//!
//! The receivers read the waveforms with the thresholds of their models, and the tests check that
//! they read what was sent, only later.
//!
//! Run with `cargo run --example ibis_signal_integrity --features ibis` (prints the results;
//! `-- --deck` prints the decks instead), and the tests with
//! `cargo test --example ibis_signal_integrity --features ibis`.

use cuprite_wire::{
    basic::Resistor0603,
    circuit::CircuitBuilder,
    ibis::Corner,
    ibis_part,
    naming::NetLabel,
    spice::{
        SpiceDeck, SpiceNetlist, TransmissionLine,
        analysis::{Probe, ProbeError, Tran},
        digital::{DigitalSequence, InputCharacteristics, Level},
        ibis::{IbisDiffDriver, IbisDiffReceiver, IbisDriver, IbisReceiver},
    },
};

ibis_part! {
    /// The I/O buffers of the made-up chip at both ends of the links.
    pub struct DemoIo {
        library: "examples/ibis_signal_integrity/demo_io.ibs",
        pins: { clk_out => "1", clk_in => "2" },
        diff_pins: { lvds_out => "3", lvds_in => "5" },
    }
}

type Error = Box<dyn std::error::Error>;

/// The corners simulated.
const CORNERS: [Corner; 3] = [Corner::Min, Corner::Typ, Corner::Max];

/// The delay of the traces.
const TRACE_DELAY: f64 = 1e-9;
/// The impedance of the clock's trace and of each trace of the pair (uncoupled, so 100 ohms
/// between them).
const Z0: f64 = 50.0;

/// A link's deck with a transient analysis recording the voltage at the receiver's input.
struct Link {
    deck: SpiceDeck,
    tran: Tran,
    /// The voltage at the receiver's pin, or between its pins.
    rx: Probe,
}

impl Link {
    /// A transient analysis of `stop` seconds of `netlist`, recording the probe made by `rx`.
    fn new(
        netlist: SpiceNetlist,
        stop: f64,
        rx: impl FnOnce(&SpiceDeck) -> Result<Probe, ProbeError>,
    ) -> Result<Self, Error> {
        let deck = netlist.deck();
        let rx = rx(&deck)?;
        let tran = Tran::new(10e-12, stop).probe(&rx);
        Ok(Link {
            deck: deck.analysis(&tran),
            tran,
            rx,
        })
    }

    /// Runs the deck: the time and the voltage at the receiver.
    fn run(&self) -> Result<(Vec<f64>, Vec<f64>), Error> {
        let result = self.deck.run()?.result(&self.tran)?;
        Ok((result.scale().to_vec(), result.get(&self.rx)?.to_vec()))
    }
}

/// Four periods of a 50 MHz clock from 5 ns.
fn clock() -> DigitalSequence {
    DigitalSequence::new(Level::Low).pwm(5e-9, 20e-9, 0.5, 4)
}

/// `bits` (`0` and `1`) one after the other from `start`, each for `period` seconds, then low.
fn pattern(start: f64, period: f64, bits: &str) -> DigitalSequence {
    let mut sequence = DigitalSequence::new(Level::Low);
    for (i, bit) in bits.chars().enumerate() {
        sequence.push(start + i as f64 * period, Level::from(bit == '1'));
    }
    sequence.at(start + bits.len() as f64 * period, Level::Low)
}

/// The bit pattern of the differential link: runs of one to four equal bits.
fn bits() -> DigitalSequence {
    pattern(4e-9, 2e-9, "0110100111100010")
}

/// The clock from `CLK_OUT` of one chip through a trace to `CLK_IN` of the other, through a
/// series resistor of `series` (KiCad's notation, `"33"`) at the driver if given. The pins of the
/// receiver is probed as `RX`.
fn clock_link(corner: Corner, series: Option<&str>) -> Result<Link, Error> {
    let mut bench = CircuitBuilder::new();
    let gnd = bench.add_node();
    bench.label(gnd, NetLabel::global("GND"));
    let driver = bench.instantiate_part(IbisDriver::new(DemoIo::clk_out(corner)?, clock()));
    let trace = bench.instantiate_part(TransmissionLine::new(Z0, TRACE_DELAY));
    let receiver = bench.instantiate_part(IbisReceiver::new(DemoIo::clk_in(corner)?));
    match series {
        Some(value) => {
            let resistor = bench.instantiate_part(Resistor0603::new(value));
            bench.connect(driver[IbisDriver::OUT], resistor["1"]);
            bench.connect(resistor["2"], trace[TransmissionLine::P1]);
        }
        None => bench.connect(driver[IbisDriver::OUT], trace[TransmissionLine::P1]),
    }
    bench.connect(trace[TransmissionLine::P2], receiver[IbisReceiver::IN]);
    for pin in [
        driver[IbisDriver::GND],
        trace[TransmissionLine::N1],
        trace[TransmissionLine::N2],
        receiver[IbisReceiver::GND],
    ] {
        bench.connect(pin, gnd);
    }
    bench.label(receiver[IbisReceiver::IN], NetLabel::local("RX"));
    let title = format!("clock link, series {}", series.unwrap_or("none"));
    let netlist = bench.build(&()).to_netlist()?.to_spice()?.with_title(title);
    Link::new(netlist, 90e-9, |deck| deck.voltage("/RX"))
}

/// The bit pattern from `TX_P`/`TX_N` of one chip through a pair of traces to `RX_P`/`RX_N` of
/// the other, where the receiver's termination joins the pins. The receiver is probed between its
/// pins.
fn differential_link(corner: Corner) -> Result<Link, Error> {
    let mut bench = CircuitBuilder::new();
    let gnd = bench.add_node();
    bench.label(gnd, NetLabel::global("GND"));
    let driver = bench.instantiate_part(IbisDiffDriver::new(DemoIo::lvds_out(corner)?, bits()));
    let receiver = bench.instantiate_part(IbisDiffReceiver::new(DemoIo::lvds_in(corner)?));
    for (from, to, probe) in [
        (IbisDiffDriver::P, IbisDiffReceiver::P, "RX_P"),
        (IbisDiffDriver::N, IbisDiffReceiver::N, "RX_N"),
    ] {
        let trace = bench.instantiate_part(TransmissionLine::new(Z0, TRACE_DELAY));
        bench.connect(driver[from], trace[TransmissionLine::P1]);
        bench.connect(trace[TransmissionLine::P2], receiver[to]);
        bench.connect(trace[TransmissionLine::N1], gnd);
        bench.connect(trace[TransmissionLine::N2], gnd);
        bench.label(receiver[to], NetLabel::local(probe));
    }
    bench.connect(driver[IbisDiffDriver::GND], gnd);
    bench.connect(receiver[IbisDiffReceiver::GND], gnd);
    let netlist = bench
        .build(&())
        .to_netlist()?
        .to_spice()?
        .with_title("differential link");
    Link::new(netlist, 40e-9, |deck| {
        deck.voltage_between("/RX_P", "/RX_N")
    })
}

/// What the clock's receiver read from the voltage `rx` at its pin over `time`, with the
/// thresholds of its model.
fn read_clock(corner: Corner, time: &[f64], rx: &[f64]) -> Result<DigitalSequence, Error> {
    let buffer = DemoIo::clk_in(corner)?;
    let input = buffer.input().ok_or("the input model has Vinl and Vinh")?;
    Ok(input.decode(time, rx))
}

/// What the differential receiver read from the voltage `rx` between its pins over `time`: high
/// above `vdiff`, low below `-vdiff`, unchanged in between.
fn read_bits(corner: Corner, time: &[f64], rx: &[f64]) -> Result<DigitalSequence, Error> {
    let vdiff = DemoIo::lvds_in(corner)?
        .vdiff()
        .ok_or("the receiver's [Diff Pin] row has vdiff")?;
    let mut comparator = InputCharacteristics::cmos(0.0);
    comparator.threshold_low = -vdiff;
    comparator.threshold_high = vdiff;
    Ok(comparator.decode(time, rx))
}

/// The delay of each change of `read` after the same change of `sent`, if `read` has the same
/// levels as `sent`.
fn delays(sent: &DigitalSequence, read: &DigitalSequence) -> Option<Vec<f64>> {
    let same = sent.initial() == read.initial()
        && sent.events().len() == read.events().len()
        && sent
            .events()
            .iter()
            .zip(read.events())
            .all(|(s, r)| s.1 == r.1);
    same.then(|| {
        sent.events()
            .iter()
            .zip(read.events())
            .map(|(s, r)| r.0 - s.0)
            .collect()
    })
}

/// The lowest and the highest of `values`.
fn range(values: &[f64]) -> (f64, f64) {
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (min, max)
}

/// Whether `read` is what was `sent`, and how much later.
fn describe(sent: &DigitalSequence, read: &DigitalSequence) -> String {
    match delays(sent, read) {
        Some(delays) => {
            let (min, max) = range(&delays);
            format!(
                "read as sent, {:.2} to {:.2} ns later",
                min * 1e9,
                max * 1e9
            )
        }
        None => format!("read wrong: {read:?}"),
    }
}

fn main() -> Result<(), Error> {
    let print_decks = std::env::args().any(|arg| arg == "--deck");
    for corner in CORNERS {
        for series in [None, Some("33")] {
            let link = clock_link(corner, series)?;
            if print_decks {
                println!("{}", link.deck);
                continue;
            }
            let (time, rx) = link.run()?;
            let (low, high) = range(&rx);
            let read = describe(&clock(), &read_clock(corner, &time, &rx)?);
            println!(
                "clock, {corner:?}, series resistor {:>4}: {low:+.2} V to {high:+.2} V, {read}",
                series.unwrap_or("none"),
            );
        }
    }
    for corner in CORNERS {
        let link = differential_link(corner)?;
        if print_decks {
            println!("{}", link.deck);
            continue;
        }
        let (time, rx) = link.run()?;
        println!(
            "bits, {corner:?}: {}",
            describe(&bits(), &read_bits(corner, &time, &rx)?)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the link: the time and the voltage at the receiver.
    fn run(link: Link) -> (Vec<f64>, Vec<f64>) {
        link.run()
            .unwrap_or_else(|error| panic!("{error}\n{}", link.deck))
    }

    /// The lowest and the highest voltage at the clock's receiver, and the delays of what it read
    /// (which must be what was sent).
    fn clock_at(corner: Corner, series: Option<&str>) -> ((f64, f64), Vec<f64>) {
        let (time, rx) = run(clock_link(corner, series).unwrap());
        let read = read_clock(corner, &time, &rx).unwrap();
        let delays = delays(&clock(), &read).unwrap_or_else(|| panic!("{corner:?}: {read:?}"));
        (range(&rx), delays)
    }

    /// Without termination, the edges reflect at the receiver and ring: they overshoot the supply
    /// until the receiver's clamp diode conducts, and undershoot the ground likewise. The clock is
    /// still read as sent, the trace's delay and part of the edge later.
    #[test]
    fn unterminated_clock_rings() {
        let ((low, high), delays) = clock_at(Corner::Typ, None);
        assert!(high > 3.3 + 0.5 && high < 3.3 + 1.0, "{high}");
        assert!(low < -0.5 && low > -1.0, "{low}");
        let (first, last) = range(&delays);
        assert!(
            first > TRACE_DELAY && last < TRACE_DELAY + 1e-9,
            "{delays:?}"
        );
    }

    /// The series resistor and the driver's own resistance match the trace: typically, the
    /// receiver stays within the rails. At every corner, the resistor cuts the overshoot and the
    /// undershoot (the stronger driver of the max corner still overshoots a little).
    #[test]
    fn series_resistor_damps_the_clock() {
        let ((low, high), _) = clock_at(Corner::Typ, Some("33"));
        assert!(high < 3.3 + 0.1 && low > -0.1, "{low} {high}");
        for corner in CORNERS {
            let ((low, high), delays) = clock_at(corner, Some("33"));
            let ((bare_low, bare_high), _) = clock_at(corner, None);
            assert!(
                high < bare_high - 0.4 && low > bare_low + 0.4,
                "{corner:?}: {low} to {high}, without the resistor {bare_low} to {bare_high}"
            );
            let (first, last) = range(&delays);
            assert!(
                first > TRACE_DELAY && last < TRACE_DELAY + 1.5e-9,
                "{corner:?}: {delays:?}"
            );
        }
    }

    /// The receiver reads the bits as sent at every corner. Typically, the pins settle 350 mV
    /// apart, as the drivers' 1.55 V and 0.85 V through about 50 ohms each into the 100 ohm
    /// termination make.
    #[test]
    fn differential_link_carries_the_bits() {
        for corner in CORNERS {
            let (time, rx) = run(differential_link(corner).unwrap());
            let read = read_bits(corner, &time, &rx).unwrap();
            let delays = delays(&bits(), &read).unwrap_or_else(|| panic!("{corner:?}: {read:?}"));
            let (first, last) = range(&delays);
            assert!(
                first > TRACE_DELAY && last < TRACE_DELAY + 0.5e-9,
                "{corner:?}: {delays:?}"
            );
        }
        let link = differential_link(Corner::Typ).unwrap();
        let result = link.deck.run().unwrap().result(&link.tran).unwrap();
        // At the end of the longest run of ones (16 to 24 ns), the trace's delay later.
        let settled = result.interpolate(&link.rx, 24.9e-9).unwrap().unwrap();
        assert!((settled - 0.35).abs() < 0.02, "{settled}");
    }
}
