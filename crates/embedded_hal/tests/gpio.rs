//! Digital pins through a periodic task.

use cuprite_wire::{
    circuit::CircuitBuilder,
    naming::NetLabel,
    spice::{
        cosim::{CosimDevice, DeviceModels},
        digital::{
            DigitalDriver, DigitalLoad, DigitalSequence, InputCharacteristics, Level,
            OutputCharacteristics,
        },
    },
};
use cuprite_wire_embedded_hal::{Board, Firmware};
use embedded_hal::digital::{InputPin, OutputPin, StatefulOutputPin};

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
    let mut bench = CircuitBuilder::new();
    let mcu = bench
        .instantiate_part(CosimDevice::new("mcu").output("out", OutputCharacteristics::cmos(3.3)));
    let load = bench.instantiate_part(DigitalLoad::new(InputCharacteristics::cmos(3.3)));
    bench.connect(mcu["out"], load[DigitalLoad::IN]);
    bench.label(mcu["out"], NetLabel::local("OUT"));
    for gnd in [mcu[CosimDevice::GND], load[DigitalLoad::GND]] {
        bench.label(gnd, NetLabel::global("GND"));
    }
    let deck = bench
        .build(&())
        .to_netlist()
        .unwrap()
        .to_spice()
        .unwrap()
        .deck()
        .control(["tran 10n 2u", "tran 10n 2u", r#"wrdata out.txt v("/OUT")"#]);
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
    let out = table.interpolate(r#"v("/OUT")"#, 1e-6).unwrap();
    assert!((out - 3.3).abs() < 1e-3, "{out}");
}

/// The pins keep their values outside the steps, and outputs start `Low`.
#[test]
fn test_pins_outside_a_run() {
    let board = Board::new();
    let mut input = board.input_pin("in");
    let mut output = board.output_pin("out");
    assert!(input.is_low().unwrap());
    assert!(output.is_set_low().unwrap());
    output.set_high().unwrap();
    assert!(output.is_set_high().unwrap());
}
