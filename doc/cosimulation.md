# Co-simulation with Rust Models

This document explains how to simulate a circuit together with a Rust model of a device that
reads and drives it, such as the control logic of a microcontroller's firmware. It needs the
`shared-ngspice` feature and the ngspice shared library (`libngspice0`); see
`doc/spice_netlist_usage.md` for test benches and running ngspice. A complete example is
`examples/pwm_dac_feedback.rs`
(`cargo run --example pwm_dac_feedback --features shared-ngspice`).

## 1. Overview

```rust
use cuprite_wire::spice::{
    cosim::{CosimDevice, DeviceIo, DeviceModel, DeviceModels},
    digital::{InputCharacteristics, Level, OutputCharacteristics},
};

struct Firmware;

impl DeviceModel for Firmware {
    fn step(&mut self, io: &mut DeviceIo<'_>) {
        // Mirror a button to an LED.
        let pressed = !io.read("button");
        io.set("led", Level::from(pressed));
    }
}

let mcu = CosimDevice::new("mcu")
    .input("button", InputCharacteristics::cmos(3.3))
    .output("led", OutputCharacteristics::cmos(3.3));
let mcu = bench.instantiate_part_named("mcu", mcu);
// ... connect mcu["button"], mcu["led"] and mcu[CosimDevice::GND], add a transient analysis ...
let mut firmware = Firmware;
let simulation = deck.run_with(DeviceModels::new().with("mcu", &mut firmware))?;
```

- `CosimDevice::new(name)` is a simulation-only part standing for the device. Its pins are added
  with `.output(pin, OutputCharacteristics)`, `.input(pin, InputCharacteristics)` and
  `.bidirectional(pin, output, input)`, plus `gnd` (`CosimDevice::GND`), the reference of the pin
  voltages. Names are ASCII letters, digits and `_`.
- A `DeviceModel` is the behavior: `step(&mut self, io)` is called at every accepted time point of
  the transient analysis, from time 0 on.
- `deck.run_with(DeviceModels::new().with(name, &mut model))` runs the deck in the shared library
  (`SharedNgspice::default().run_with(...)` for another library file) and returns the same
  `Simulation` as `run()`. Models are bound to devices by name. Every device needs a model and every
  model a device; a mismatch is `SimulationError::Cosimulation`.

## 2. Writing a model

`DeviceIo` gives the model the state of the pins at the accepted point:

| Method | |
|---|---|
| `time()` | the time of the point, in seconds |
| `voltage(pin)` | the voltage of the pin relative to `gnd` (an analog input) |
| `read(pin)` | the level an input reads, with its thresholds and hysteresis |
| `set(pin, Level)` | drives an output `Low`, `High` or `HiZ` |
| `set_voltage(pin, volts)` | drives an output to a voltage (a DAC) |
| `wake_at(time)` | asks for an accepted point at `time` |

- **Change state only in `step`.** ngspice also evaluates the outputs at trial time points it may
  reject; the outputs follow what `step` set, so a rejected point cannot change the model.
- **Outputs change at accepted points.** A change set in `step` starts at the time of that point,
  with the edges of the pin's `OutputCharacteristics` (the rise and fall times scaled to the swing;
  changes to and from `HiZ` switch the output resistance). The library asks ngspice for a time point
  at the end of each edge.
- **Ask for the times you need.** Between accepted points the model is not called. A clock, a
  timer or a sampling instant needs `wake_at(time)` (a breakpoint of ngspice), or the change lands
  on whatever point comes next. Reacting to an input happens at the first accepted point after it
  crosses the threshold, so the reaction time depends on the time step (`tran <step>`, `.options`).
- Outputs start `Low` and bidirectional pins `HiZ` until the model sets them. The operating point
  before the transient analysis is computed with these levels. Other analyses (`op`, `ac`, `dc`) see
  the outputs as they are and do not call the models; use one transient analysis per run.
- A panic in a model (e.g. an `assert!` or an unknown pin name) is resumed by `run_with` after the
  run, so it fails the test as usual; the models are not called after it.

## 3. Peripherals

`cuprite_wire::spice::cosim::peripherals` has models of common peripherals, called from `step`:

- `Periodic::new(period, start)`: `due(io)` is true once per period and asks for the next time point.
  Use it for timer interrupts and control loops.
- `Adc::new(pin, bits, reference)`: `read(io)` converts the pin voltage
  (`floor(v / reference * 2^bits)`, clamped); `voltage_of(code)` converts back.
- `Pwm::new(pin, period, start)`: `set_duty(duty)` takes effect at the next period;
  call `update(io)` at every step.

```rust
impl DeviceModel for Controller {
    fn step(&mut self, io: &mut DeviceIo<'_>) {
        if self.control.due(io) {
            let measured = self.adc.voltage_of(self.adc.read(io));
            self.pwm.set_duty(self.pwm.duty() + 0.05 * (TARGET - measured));
        }
        self.pwm.update(io);
    }
}
```

Models of a particular microcontroller (its registers, its ADC timing) belong to the user's code;
build them on these.

## 4. Firmware written with embedded-hal

The crate `cuprite_wire_embedded_hal` (`crates/embedded_hal`) runs the control logic of a firmware
written against the `embedded-hal` 1.0 traits, unchanged, as the model of a device. A complete
example is `crates/embedded_hal/examples/pwm_dac_firmware.rs`
(`cargo run -p cuprite_wire_embedded_hal --example pwm_dac_firmware`): the regulator of section 3
as a firmware module that depends only on `core` and `embedded-hal`.

```rust
use cuprite_wire_embedded_hal::{Board, Firmware};

let board = Board::new();
let regulator = Regulator::new(board.pwm("pwm", 10e-6, 1000), board.adc("adc", 12, 3.3));
let mut mcu = Firmware::new(&board, 20e-6, regulator, Regulator::step);
deck.run_with(DeviceModels::new().with("mcu", &mut mcu))?;
let regulator = mcu.into_task();   // check its state after the run
```

- `Board` hands out the pins of the `CosimDevice` as `embedded-hal` objects:
  `input_pin(pin)` (`InputPin`), `output_pin(pin)` (`OutputPin`, `StatefulOutputPin`),
  `pwm(pin, period, max_duty)` (`SetDutyCycle`; a new duty takes effect at the next period) and
  `adc(pin, bits, reference)` (`AdcChannel`). The errors are `Infallible`.
- **The firmware runs as a periodic task.** `Firmware::new(&board, period, task, step)` calls
  `step(&mut task)` every `period` seconds from time 0, as a timer interrupt or the body of a super
  loop would. At each call the inputs and ADC codes are those of that time, and the outputs set
  during the call change from that time on. Blocking waits (`DelayNs`) cannot advance the simulated
  time from inside a call, so they are not provided: write waits as states of the task.
- **ADC.** `embedded-hal` 1.0 has no ADC trait. The crate has a minimal one, `AdcChannel`
  (`read() -> u16`), but a firmware built for the target cannot depend on the crate: it defines
  its own trait, implemented by its HAL on the target and, in the test, for `AdcPin` in one line
  (`impl AnalogInput for AdcPin { fn read(&mut self) -> u16 { AdcChannel::read(self) } }`).

## 5. The netlist and the run

The device is written as a subcircuit `cuprite_cosim_<name>`, so `deck.cir` in the run directory
shows what ngspice simulates:

```text
.subckt cuprite_cosim_mcu button led gnd
CC_button button gnd 5e-12
VD_led d_led gnd DC 0 EXTERNAL
VE_led e_led gnd DC 0 EXTERNAL
SO_led d_led led e_led gnd hiz_led
.model hiz_led SW( vt=0.5 vh=0 ron=50 roff=1e9 )
.ends cuprite_cosim_mcu
```

An output pin is the `EXTERNAL` source `VD_<pin>` (its voltage) behind a switch with the output
resistance, closed by the `EXTERNAL` source `VE_<pin>` (1 while driving, 0 in `HiZ`). An input pin
has the elements of `DigitalLoad` (capacitance, pull resistor, leakage). The values of the
`EXTERNAL` sources come from the models through the callbacks of the shared library, so the deck
cannot be rerun with the CLI as it is (the sources stay at 0 V).

The run has the limits of the shared library (`doc/spice_netlist_usage.md` section 5): one run at a
time in the process, and a crash of ngspice ends the test process.

**Speed.** Every edge of an output adds time points. Simulating milliseconds of a fast PWM is
quick (the example runs 3 ms with a 100 kHz PWM in a fraction of a second), but long stretches of
firmware time are not: test the interval that matters, with the state the model starts from.
