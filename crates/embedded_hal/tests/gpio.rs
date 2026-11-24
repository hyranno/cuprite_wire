//! Digital pins through a periodic task.

use cuprite_wire::{
    circuit::CircuitBuilder,
    naming::NetLabel,
    spice::{
        SpiceDeck,
        cosim::{CosimDevice, DeviceModels},
        digital::{
            DigitalDriver, DigitalLoad, DigitalSequence, InputCharacteristics, Level,
            OutputCharacteristics, Pull,
        },
    },
};
use cuprite_wire_embedded_hal::{Board, Firmware};
use embedded_hal::{
    digital::{InputPin, OutputPin, StatefulOutputPin},
    pwm::SetDutyCycle,
};

/// Copies an input to an output at each step, counting the steps.
struct Mirror<I, O> {
    input: I,
    output: O,
    steps: usize,
}

impl<I: InputPin, O: StatefulOutputPin> Mirror<I, O> {
    fn step(&mut self) {
        let high = self.input.is_high().unwrap();
        if high != self.output.is_set_high().unwrap() {
            self.output.set_state(high.into()).unwrap();
        }
        self.steps += 1;
    }
}

/// The output follows the input a step later: the input is sampled every 1 us.
#[test]
fn test_mirror() {
    let mut bench = CircuitBuilder::new();
    let sent = DigitalSequence::new(Level::Low)
        .at(2.5e-6, Level::High)
        .at(6.5e-6, Level::Low);
    let driver = bench.instantiate_part(DigitalDriver::new(OutputCharacteristics::cmos(3.3), sent));
    let mcu = bench.instantiate_part(
        CosimDevice::new("mcu")
            .input("in", InputCharacteristics::cmos(3.3))
            .output("out", OutputCharacteristics::cmos(3.3)),
    );
    let input = InputCharacteristics::cmos(3.3);
    let load = bench.instantiate_part(DigitalLoad::new(input.clone()));
    bench.connect(driver[DigitalDriver::OUT], mcu["in"]);
    bench.connect(mcu["out"], load[DigitalLoad::IN]);
    bench.label(mcu["out"], NetLabel::local("OUT"));
    for gnd in [
        driver[DigitalDriver::GND],
        mcu[CosimDevice::GND],
        load[DigitalLoad::GND],
    ] {
        bench.label(gnd, NetLabel::global("GND"));
    }
    let deck = bench
        .build(&())
        .to_netlist()
        .unwrap()
        .to_spice()
        .unwrap()
        .deck()
        .control(["tran 10n 10u", r#"wrdata out.txt v("/OUT")"#]);

    let board = Board::new();
    let mirror = Mirror {
        input: board.input_pin("in"),
        output: board.output_pin("out"),
        steps: 0,
    };
    let mut firmware = Firmware::new(&board, 1e-6, mirror, Mirror::step);
    let sim = deck
        .run_with(DeviceModels::new().with("mcu", &mut firmware))
        .unwrap();
    // Steps at 0, 1, ..., 10 us.
    assert_eq!(firmware.task().steps, 11);
    let table = sim.table("out.txt").unwrap();
    let read = input.decode(table.scale(), table.column(r#"v("/OUT")"#).unwrap());
    let [(rise, Level::High), (fall, Level::Low)] = read.events() else {
        panic!("{read:?}");
    };
    assert!((rise - 3e-6).abs() < 10e-9, "{rise}");
    assert!((fall - 7e-6).abs() < 10e-9, "{fall}");
}

/// Two transient analyses in one run: the firmware runs from time 0 in each, and the output it
/// set in the first is driven again in the second.
#[test]
fn test_two_transient_analyses() {
    struct Once<O> {
        output: O,
        steps: usize,
    }
    fn step<O: OutputPin>(task: &mut Once<O>) {
        if task.steps == 0 {
            task.output.set_high().unwrap();
        }
        task.steps += 1;
    }
    let deck = output_deck(&["tran 10n 2u", "tran 10n 2u"]);
    let board = Board::new();
    let once = Once {
        output: board.output_pin("out"),
        steps: 0,
    };
    let mut firmware = Firmware::new(&board, 1e-6, once, step);
    let sim = deck
        .run_with(DeviceModels::new().with("mcu", &mut firmware))
        .unwrap();
    // Steps at 0, 1 and 2 us in each analysis.
    assert_eq!(firmware.task().steps, 6);
    let table = sim.table("out.txt").unwrap();
    let out = table.interpolate(OUT, 1e-6).unwrap();
    assert!((out - 3.3).abs() < 1e-3, "{out}");
}

const OUT: &str = r#"v("/OUT")"#;

/// A device with the output `out` on the net `/OUT`, loaded by an input; the deck runs `analyses`
/// and writes the voltage of `/OUT` to `out.txt`.
fn output_deck(analyses: &[&str]) -> SpiceDeck {
    let mut bench = CircuitBuilder::new();
    let mcu = bench
        .instantiate_part(CosimDevice::new("mcu").output("out", OutputCharacteristics::cmos(3.3)));
    let load = bench.instantiate_part(DigitalLoad::new(InputCharacteristics::cmos(3.3)));
    bench.connect(mcu["out"], load[DigitalLoad::IN]);
    bench.label(mcu["out"], NetLabel::local("OUT"));
    for gnd in [mcu[CosimDevice::GND], load[DigitalLoad::GND]] {
        bench.label(gnd, NetLabel::global("GND"));
    }
    bench
        .build(&())
        .to_netlist()
        .unwrap()
        .to_spice()
        .unwrap()
        .deck()
        .control(analyses.iter().copied())
        .control([format!("wrdata out.txt {OUT}")])
}

/// An open-drain line with a pull-up, shared by the firmware and another open-drain driver: the
/// line is low while either pulls it low, and the firmware reads it whatever drives it.
#[test]
fn test_open_drain_wired_and() {
    struct Pulse<P> {
        line: P,
        reads: Vec<bool>,
        released: Vec<bool>,
    }
    fn step<P: InputPin + StatefulOutputPin>(task: &mut Pulse<P>) {
        task.reads.push(task.line.is_high().unwrap());
        task.released.push(task.line.is_set_high().unwrap());
        // Pulls the line low at the step at 2 us, releases it at 3 us.
        match task.reads.len() {
            3 => task.line.set_low().unwrap(),
            4 => task.line.set_high().unwrap(),
            _ => {}
        }
    }
    let mut bench = CircuitBuilder::new();
    let output = OutputCharacteristics::cmos(3.3);
    // The other device pulls the line low from 4.5 to 6.5 us.
    let other = DigitalSequence::new(Level::HiZ)
        .at(4.5e-6, Level::Low)
        .at(6.5e-6, Level::HiZ);
    let driver = bench.instantiate_part(DigitalDriver::new(output.clone(), other));
    let pull_up = InputCharacteristics::cmos(3.3).with_pull(Pull::up(3.3, 4.7e3));
    let mcu = bench.instantiate_part(CosimDevice::new("mcu").bidirectional("sda", output, pull_up));
    bench.connect(driver[DigitalDriver::OUT], mcu["sda"]);
    for gnd in [driver[DigitalDriver::GND], mcu[CosimDevice::GND]] {
        bench.label(gnd, NetLabel::global("GND"));
    }
    let deck = bench
        .build(&())
        .to_netlist()
        .unwrap()
        .to_spice()
        .unwrap()
        .deck()
        .control(["tran 10n 8u"]);

    let board = Board::new();
    let pulse = Pulse {
        line: board.open_drain_pin("sda"),
        reads: Vec::new(),
        released: Vec::new(),
    };
    let mut firmware = Firmware::new(&board, 1e-6, pulse, step);
    deck.run_with(DeviceModels::new().with("mcu", &mut firmware))
        .unwrap();
    let pulse = firmware.into_task();
    // Steps at 0, 1, ..., 8 us: low at 3 us by the firmware, at 5 and 6 us by the other device.
    let reads = [true, true, true, false, true, false, false, true, true];
    assert_eq!(pulse.reads, reads);
    let released = [true, true, true, false, true, true, true, true, true];
    assert_eq!(pulse.released, released);
}

/// A duty set by a step at the time a PWM period starts takes effect at the next period, as with
/// the buffered compare register of a timer whose interrupt is at the start of the period.
#[test]
fn test_pwm_duty_at_period_start() {
    let deck = output_deck(&["tran 10n 3u"]);
    let board = Board::new();
    let mut firmware = Firmware::new(&board, 1e-6, board.pwm("out", 1e-6, 4), |pwm| {
        pwm.set_duty_cycle(2).unwrap();
    });
    let sim = deck
        .run_with(DeviceModels::new().with("mcu", &mut firmware))
        .unwrap();
    let table = sim.table("out.txt").unwrap();
    let at = |t| table.interpolate(OUT, t).unwrap();
    // The first period has the duty of 0 the channel starts with.
    assert!(at(0.25e-6) < 0.1, "{}", at(0.25e-6));
    assert!(at(0.75e-6) < 0.1, "{}", at(0.75e-6));
    for period in [1e-6, 2e-6] {
        let (high, low) = (at(period + 0.25e-6), at(period + 0.75e-6));
        assert!(high > 3.2 && low < 0.1, "{period}: {high} {low}");
    }
}

/// The pins keep their values outside the steps, and outputs are neither set high nor low until
/// they are set (a bidirectional pin starts in `HiZ`).
#[test]
fn test_pins_outside_a_run() {
    let board = Board::new();
    let mut input = board.input_pin("in");
    let mut output = board.output_pin("out");
    assert!(input.is_low().unwrap());
    assert!(!output.is_set_low().unwrap());
    assert!(!output.is_set_high().unwrap());
    output.set_high().unwrap();
    assert!(output.is_set_high().unwrap());
    assert!(!output.is_set_low().unwrap());
    output.set_low().unwrap();
    assert!(output.is_set_low().unwrap());
    // An open-drain pin starts released.
    let mut line = board.open_drain_pin("sda");
    assert!(line.is_set_high().unwrap());
    line.set_low().unwrap();
    assert!(line.is_set_low().unwrap());
    assert!(!line.is_set_high().unwrap());
}
