//! Co-simulation runs with small models.

use super::{CosimDevice, DeviceIo, DeviceModel, DeviceModels, PinIndex, PinState};
use crate::{
    circuit::CircuitBuilder,
    naming::NetLabel,
    spice::{
        IndependentSource, SimulationError, SpiceDeck, Table,
        digital::{
            DigitalDriver, DigitalLoad, DigitalSequence, InputCharacteristics, Level,
            OutputCharacteristics, Pull,
        },
    },
    test_parts::deck_of,
};

const LINE: &str = r#"v("/LINE")"#;

/// `device` with its pin `pin` on the line `/LINE`, loaded by `load`; the deck runs a transient
/// analysis of `stop` seconds and writes the voltage of the line.
fn line_deck(device: Box<CosimDevice>, pin: &str, load: Box<DigitalLoad>, stop: f64) -> SpiceDeck {
    let mut circuit = CircuitBuilder::new();
    let device = circuit.instantiate_part(device);
    let load = circuit.instantiate_part(load);
    circuit.connect(device[pin], load[DigitalLoad::IN]);
    circuit.label(device[pin], NetLabel::local("LINE"));
    circuit.label(device[CosimDevice::GND], NetLabel::global("GND"));
    circuit.label(load[DigitalLoad::GND], NetLabel::global("GND"));
    deck_of(&circuit).control([
        format!("tran 10n {stop:e}"),
        format!("wrdata line.txt {LINE}"),
    ])
}

fn table(deck: &SpiceDeck, models: DeviceModels<'_>) -> Table {
    deck.run_with(models).unwrap().table("line.txt").unwrap()
}

/// Sets `out` to the levels of a sequence at its times, waking up for each.
struct Player {
    sequence: DigitalSequence,
    steps: usize,
}

impl DeviceModel for Player {
    fn step(&mut self, io: &mut DeviceIo<'_>) {
        self.steps += 1;
        let time = io.time();
        io.set("out", self.sequence.level_at(time));
        if let Some(&(next, _)) = self.sequence.events().iter().find(|e| e.0 > time) {
            io.wake_at(next);
        }
    }
}

/// An output switched at 1 us through 1 kohm into 1 nF: the rise starts at the breakpoint, so the
/// input threshold (70 %) is crossed after ln(1 / 0.3) time constants.
#[test]
fn test_output_at_breakpoint() {
    let input = InputCharacteristics::cmos(3.3).with_capacitance(1e-9);
    let device = CosimDevice::new("mcu")
        .output("out", OutputCharacteristics::cmos(3.3).with_resistance(1e3));
    let deck = line_deck(device, "out", DigitalLoad::new(input.clone()), 6e-6);
    let mut player = Player {
        sequence: DigitalSequence::new(Level::Low).at(1e-6, Level::High),
        steps: 0,
    };
    let table = table(&deck, DeviceModels::new().with("mcu", &mut player));
    assert!(player.steps > 10, "{}", player.steps);
    let read = input.decode(table.scale(), table.column(LINE).unwrap());
    let [(t, Level::High)] = read.events() else {
        panic!("{read:?}");
    };
    assert!((t - 2.204e-6).abs() < 5e-9, "{t}");
}

/// An open-drain output (`HiZ` / `Low`) with a pull-up, on a bidirectional pin that reads the line
/// back: released at 1 us, pulled low at 5 us.
#[test]
fn test_open_drain_reads_back() {
    struct OpenDrain {
        reads: Vec<(f64, bool)>,
    }
    impl DeviceModel for OpenDrain {
        fn step(&mut self, io: &mut DeviceIo<'_>) {
            let time = io.time();
            let level = if (1e-6..5e-6).contains(&time) {
                Level::HiZ
            } else {
                Level::Low
            };
            io.set("sda", level);
            io.wake_at(1e-6);
            io.wake_at(5e-6);
            let read = io.read("sda");
            if self.reads.last().is_none_or(|&(_, last)| last != read) {
                self.reads.push((time, read));
            }
        }
    }
    let output = OutputCharacteristics::cmos(3.3);
    let input = InputCharacteristics::cmos(3.3);
    let device = CosimDevice::new("mcu").bidirectional("sda", output, input);
    let pull_up = InputCharacteristics::cmos(3.3)
        .with_capacitance(100e-12)
        .with_pull(Pull::up(3.3, 10e3));
    let deck = line_deck(device, "sda", DigitalLoad::new(pull_up), 6e-6);
    let mut model = OpenDrain { reads: Vec::new() };
    let table = table(&deck, DeviceModels::new().with("mcu", &mut model));
    let at = |t| table.interpolate(LINE, t).unwrap();
    assert!(at(0.5e-6) < 0.05, "{}", at(0.5e-6));
    // 10 kohm and about 105 pF (the load and the input of the pin).
    assert!((at(2e-6) - 3.3 * 0.614).abs() < 0.05, "{}", at(2e-6));
    assert!(at(5.2e-6) < 0.05, "{}", at(5.2e-6));
    // The pin starts in `HiZ`, so it reads the pull-up until the model drives it low.
    let [(0.0, true), (low, false), (rise, true), (fall, false)] = model.reads[..] else {
        panic!("{:?}", model.reads);
    };
    assert!(low < 0.1e-6, "{low}");
    assert!((rise - 2.2e-6).abs() < 0.1e-6, "{rise}");
    assert!((fall - 5e-6).abs() < 0.05e-6, "{fall}");
}

/// A model echoing an input to an output: the output follows the edges of a driver.
#[test]
fn test_echo_input() {
    struct Echo;
    impl DeviceModel for Echo {
        fn step(&mut self, io: &mut DeviceIo<'_>) {
            let level = io.read("in").into();
            io.set("out", level);
        }
    }
    let mut circuit = CircuitBuilder::new();
    let sent = DigitalSequence::new(Level::Low).pwm(1e-6, 2e-6, 0.5, 3);
    let driver = circuit.instantiate_part(DigitalDriver::new(
        OutputCharacteristics::cmos(3.3),
        sent.clone(),
    ));
    let device = circuit.instantiate_part(
        CosimDevice::new("mcu")
            .input("in", InputCharacteristics::cmos(3.3))
            .output("out", OutputCharacteristics::cmos(3.3)),
    );
    let input = InputCharacteristics::cmos(3.3);
    let load = circuit.instantiate_part(DigitalLoad::new(input.clone()));
    circuit.connect(driver[DigitalDriver::OUT], device["in"]);
    circuit.connect(device["out"], load[DigitalLoad::IN]);
    circuit.label(device["out"], NetLabel::local("LINE"));
    for gnd in [
        driver[DigitalDriver::GND],
        device[CosimDevice::GND],
        load[DigitalLoad::GND],
    ] {
        circuit.label(gnd, NetLabel::global("GND"));
    }
    let deck =
        deck_of(&circuit).control(["tran 10n 8u".to_string(), format!("wrdata line.txt {LINE}")]);
    let table = table(&deck, DeviceModels::new().with("mcu", &mut Echo));
    let read = input.decode(table.scale(), table.column(LINE).unwrap());
    assert_eq!(read.events().len(), sent.events().len(), "{read:?}");
    for (echo, edge) in read.events().iter().zip(sent.events()) {
        assert_eq!(echo.1, edge.1);
        // The echo starts at the first accepted point after the input crosses its threshold.
        assert!(
            echo.0 > edge.0 && echo.0 - edge.0 < 50e-9,
            "{echo:?} {edge:?}"
        );
    }
}

/// A model counting the edges of an input, with the pin looked up once: the edges are seen at the
/// first accepted point after the input crosses its threshold.
#[test]
fn test_edges() {
    struct Edges {
        clk: PinIndex,
        rises: Vec<f64>,
        falls: Vec<f64>,
    }
    impl DeviceModel for Edges {
        fn step(&mut self, io: &mut DeviceIo<'_>) {
            if io.rose(self.clk) {
                self.rises.push(io.time());
            }
            if io.fell(self.clk) {
                self.falls.push(io.time());
            }
        }
    }
    let mut circuit = CircuitBuilder::new();
    let sent = DigitalSequence::new(Level::Low).pwm(1e-6, 2e-6, 0.5, 3);
    let driver = circuit.instantiate_part(DigitalDriver::new(
        OutputCharacteristics::cmos(3.3),
        sent.clone(),
    ));
    let device = CosimDevice::new("mcu").input("clk", InputCharacteristics::cmos(3.3));
    let mut model = Edges {
        clk: device.pin_index("CLK"),
        rises: Vec::new(),
        falls: Vec::new(),
    };
    let device = circuit.instantiate_part(device);
    circuit.connect(driver[DigitalDriver::OUT], device["clk"]);
    for gnd in [driver[DigitalDriver::GND], device[CosimDevice::GND]] {
        circuit.label(gnd, NetLabel::global("GND"));
    }
    let deck = deck_of(&circuit).control(["tran 10n 8u".to_string()]);
    deck.run_with(DeviceModels::new().with("mcu", &mut model))
        .unwrap();
    let edges = |level| -> Vec<f64> {
        sent.events()
            .iter()
            .filter(|e| e.1 == level)
            .map(|e| e.0)
            .collect()
    };
    for (seen, sent) in [
        (&model.rises, edges(Level::High)),
        (&model.falls, edges(Level::Low)),
    ] {
        assert_eq!(seen.len(), sent.len(), "{seen:?} {sent:?}");
        for (seen, sent) in seen.iter().zip(&sent) {
            assert!(seen > sent && seen - sent < 50e-9, "{seen} {sent}");
        }
    }
}

/// `DeviceIo` without a run: buses and pins by index.
#[test]
fn test_buses() {
    let device = CosimDevice::new("mcu")
        .input("d0", InputCharacteristics::cmos(3.3))
        .input("d1", InputCharacteristics::cmos(3.3))
        .input("d2", InputCharacteristics::cmos(3.3))
        .output("q0", OutputCharacteristics::cmos(3.3))
        .output("q1", OutputCharacteristics::cmos(3.3));
    let mut pins: Vec<PinState> = device.pins().iter().cloned().map(PinState::new).collect();
    for (pin, read) in pins.iter_mut().zip([true, false, true]) {
        pin.read = read;
    }
    let mut wakes = Vec::new();
    let mut io = DeviceIo {
        time: 0.0,
        first: false,
        pins: &mut pins,
        wakes: &mut wakes,
    };
    assert_eq!(io.read_bus(["d0", "d1", "d2"]), 0b101);
    assert_eq!(io.read_bus(["d2", "d1", "d0"]), 0b101);
    let names: Vec<String> = ["d1", "d2"].map(String::from).into();
    assert_eq!(io.read_bus(&names), 0b10);
    let q = [device.pin_index("q0"), io.pin_index("Q1")];
    assert_eq!(q, [PinIndex(3), PinIndex(4)]);
    io.set_bus(q, 0b10);
    use super::Drive::Level as L;
    let requests: Vec<_> = pins[3..].iter().map(|p| p.request).collect();
    assert_eq!(requests, [Some(L(Level::Low)), Some(L(Level::High))]);
}

#[test]
#[should_panic(expected = "does not fit in 2 pins")]
fn test_bus_overflow() {
    let device = CosimDevice::new("mcu")
        .output("q0", OutputCharacteristics::cmos(3.3))
        .output("q1", OutputCharacteristics::cmos(3.3));
    let mut pins: Vec<PinState> = device.pins().iter().cloned().map(PinState::new).collect();
    let mut wakes = Vec::new();
    let mut io = DeviceIo {
        time: 0.0,
        first: false,
        pins: &mut pins,
        wakes: &mut wakes,
    };
    io.set_bus(["q0", "q1"], 4);
}

#[test]
#[should_panic(expected = "the voltage of the pin \"q0\" must be between 0 V and 3.3 V: NaN")]
fn test_set_voltage_out_of_range() {
    let device = CosimDevice::new("mcu").output("q0", OutputCharacteristics::cmos(3.3));
    let mut pins: Vec<PinState> = device.pins().iter().cloned().map(PinState::new).collect();
    let mut wakes = Vec::new();
    let mut io = DeviceIo {
        time: 0.0,
        first: false,
        pins: &mut pins,
        wakes: &mut wakes,
    };
    io.set_voltage("q0", 3.3);
    io.set_voltage("q0", 0.0);
    io.set_voltage("q0", f64::NAN);
}

#[test]
#[should_panic(expected = "the device \"mcu\" has no pin \"x\"")]
fn test_no_pin_index() {
    let _ = CosimDevice::new("mcu")
        .output("q0", OutputCharacteristics::cmos(3.3))
        .pin_index("x");
}

#[test]
fn test_models_must_match_devices() {
    let device = CosimDevice::new("mcu").output("out", OutputCharacteristics::cmos(3.3));
    let deck = line_deck(
        device,
        "out",
        DigitalLoad::new(InputCharacteristics::cmos(3.3)),
        1e-6,
    );
    let error = |models| match deck.run_with(models) {
        Err(SimulationError::Cosimulation(reason)) => reason,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        error(DeviceModels::new()),
        "no model is given for the device \"mcu\""
    );
    let mut player = Player {
        sequence: DigitalSequence::new(Level::Low),
        steps: 0,
    };
    let mut other = Player {
        sequence: DigitalSequence::new(Level::Low),
        steps: 0,
    };
    assert_eq!(
        error(
            DeviceModels::new()
                .with("mcu", &mut player)
                .with("dsp", &mut other)
        ),
        "the circuit has no device \"dsp\" for a model"
    );
}

/// A panic in a model is resumed after the run, and the next run works.
#[test]
fn test_model_panics() {
    struct Panics;
    impl DeviceModel for Panics {
        fn step(&mut self, io: &mut DeviceIo<'_>) {
            io.set("no_such_pin", Level::High);
        }
    }
    let device = CosimDevice::new("mcu").output("out", OutputCharacteristics::cmos(3.3));
    let deck = line_deck(
        device,
        "out",
        DigitalLoad::new(InputCharacteristics::cmos(3.3)),
        1e-6,
    );
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        deck.run_with(DeviceModels::new().with("mcu", &mut Panics))
    }))
    .unwrap_err();
    let message = panic.downcast_ref::<String>().unwrap();
    assert!(message.contains("no pin \"no_such_pin\""), "{message}");
    let mut player = Player {
        sequence: DigitalSequence::new(Level::Low),
        steps: 0,
    };
    deck.run_with(DeviceModels::new().with("mcu", &mut player))
        .unwrap();
    assert!(player.steps > 0);
}

/// With the device's `gnd` 1 V above the ground, the pins read and drive relative to it.
#[test]
fn test_reference_above_ground() {
    struct Probe {
        sense: f64,
    }
    impl DeviceModel for Probe {
        fn step(&mut self, io: &mut DeviceIo<'_>) {
            self.sense = io.voltage("sense");
            io.set("out", Level::High);
        }
    }
    let mut circuit = CircuitBuilder::new();
    let device = circuit.instantiate_part(
        CosimDevice::new("mcu")
            .input("sense", InputCharacteristics::cmos(3.3))
            .output("out", OutputCharacteristics::cmos(3.3)),
    );
    let offset = circuit.instantiate_part(IndependentSource::voltage(1.0));
    let sense = circuit.instantiate_part(IndependentSource::voltage(2.5));
    circuit.connect(device["sense"], sense[IndependentSource::P]);
    circuit.label(device["out"], NetLabel::local("LINE"));
    for vss in [device[CosimDevice::GND], offset[IndependentSource::P]] {
        circuit.label(vss, NetLabel::local("VSS"));
    }
    for gnd in [offset[IndependentSource::N], sense[IndependentSource::N]] {
        circuit.label(gnd, NetLabel::global("GND"));
    }
    let deck =
        deck_of(&circuit).control(["tran 10n 1u".to_string(), format!("wrdata line.txt {LINE}")]);
    let mut probe = Probe { sense: 0.0 };
    let table = table(&deck, DeviceModels::new().with("mcu", &mut probe));
    assert!((probe.sense - 1.5).abs() < 1e-6, "{}", probe.sense);
    let end = *table.scale().last().unwrap();
    let out = table.interpolate(LINE, end).unwrap();
    assert!((out - 4.3).abs() < 1e-3, "{out}");
}

/// Two transient analyses in one run: the time goes back to 0 at the second one, and a periodic
/// task starts over.
#[test]
fn test_two_transient_analyses() {
    struct Tick {
        timer: super::peripherals::Periodic,
        runs: Vec<f64>,
        firsts: usize,
    }
    impl DeviceModel for Tick {
        fn step(&mut self, io: &mut DeviceIo<'_>) {
            if io.is_first_point() {
                self.firsts += 1;
            }
            if self.timer.due(io) {
                self.runs.push(io.time());
            }
        }
    }
    let device = CosimDevice::new("mcu").output("out", OutputCharacteristics::cmos(3.3));
    let deck = line_deck(
        device,
        "out",
        DigitalLoad::new(InputCharacteristics::cmos(3.3)),
        1e-6,
    )
    .control(["tran 10n 1u"]);
    let mut tick = Tick {
        timer: super::peripherals::Periodic::new(0.4e-6, 0.0),
        runs: Vec::new(),
        firsts: 0,
    };
    deck.run_with(DeviceModels::new().with("mcu", &mut tick))
        .unwrap();
    assert_eq!(tick.firsts, 2);
    let [0.0, a, b, 0.0, c, d] = tick.runs[..] else {
        panic!("{:?}", tick.runs);
    };
    for (run, expected) in [(a, 0.4e-6), (b, 0.8e-6), (c, 0.4e-6), (d, 0.8e-6)] {
        assert!((run - expected).abs() < 1e-12, "{:?}", tick.runs);
    }
}

/// With `save` leaving out the node of a pin, the pin cannot be read: an error, not 0 V.
#[test]
fn test_pin_not_saved() {
    let mut circuit = CircuitBuilder::new();
    let device = circuit
        .instantiate_part(CosimDevice::new("mcu").input("sense", InputCharacteristics::cmos(3.3)));
    let sense = circuit.instantiate_part(IndependentSource::voltage(2.5));
    circuit.connect(device["sense"], sense[IndependentSource::P]);
    circuit.label(device["sense"], NetLabel::local("SENSE"));
    for gnd in [device[CosimDevice::GND], sense[IndependentSource::N]] {
        circuit.label(gnd, NetLabel::global("GND"));
    }
    let deck = deck_of(&circuit).control([r#"save i(V1)"#, "tran 10n 1u"]);
    struct Probe;
    impl DeviceModel for Probe {
        fn step(&mut self, io: &mut DeviceIo<'_>) {
            panic!("called with {} V", io.voltage("sense"));
        }
    }
    match deck.run_with(DeviceModels::new().with("mcu", &mut Probe)) {
        Err(SimulationError::Cosimulation(reason)) => {
            assert!(reason.contains("/sense"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
}
