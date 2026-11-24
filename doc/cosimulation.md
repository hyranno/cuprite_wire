# Co-simulation with Rust Models

This document explains how to simulate a circuit together with a model of a device that reads and
drives it: a Rust model, such as the control logic of a microcontroller's firmware (sections 1-5),
or a digital model compiled into a shared library, such as an HDL design (section 6).
Rust models need the `shared-ngspice` feature and the ngspice shared library (`libngspice0`); see
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
let mcu = bench.instantiate_part(Named("mcu", mcu));
// ... connect mcu["button"], mcu["led"] and mcu[CosimDevice::GND], add a transient analysis ...
let mut firmware = Firmware;
let simulation = deck.run_with(DeviceModels::new().with("mcu", &mut firmware))?;
```

- `CosimDevice::new(name)` is a simulation-only part standing for the device. Its pins are added
  with `.output(pin, OutputCharacteristics)`, `.input(pin, InputCharacteristics)` and
  `.bidirectional(pin, output, input)`, plus `gnd` (`CosimDevice::GND`), the reference of the pin
  voltages. Names are ASCII letters, digits and `_`; pin names are unique case-insensitively and
  cannot be `gnd`, `ref` or `0` (nodes of the generated subcircuit).
- A `DeviceModel` is the behavior: `step(&mut self, io)` is called at every accepted time point of
  the transient analysis, from time 0 on. The models need the points ngspice computes, from time 0:
  `tran` with a `tstart` and `option interp` are `SimulationError::Cosimulation`. The commands of
  the background thread (`bg_run`, ...) are not supported (`SimulationError::SharedLibrary`).
- `deck.run_with(DeviceModels::new().with(name, &mut model))` runs the deck in the shared library
  (`SharedNgspice::new(path).run_with(...)` for another library file) and returns the same
  `Simulation` as `run()`. Models are bound to devices by name. Every device needs a model and every
  model a device; a mismatch is `SimulationError::Cosimulation`.

## 2. Writing a model

`DeviceIo` gives the model the state of the pins at the accepted point:

| Method | |
|---|---|
| `time()` | the time of the point, in seconds |
| `is_first_point()` | whether this is the first point of the transient analysis (time 0) |
| `voltage(pin)` | the voltage of the pin relative to `gnd` (an analog input) |
| `read(pin)` | the level an input reads, with its thresholds and hysteresis |
| `rose(pin)` / `fell(pin)` | whether the input changed to high / low since the accepted point before |
| `read_bus(pins)` | the levels of the inputs as a `u32`, the first pin as bit 0 |
| `set(pin, Level)` | drives an output `Low`, `High` or `HiZ` |
| `set_voltage(pin, volts)` | drives an output to a voltage between its `low` and `high` levels (a DAC) |
| `set_bus(pins, value)` | drives the outputs to the bits of `value`, the first pin as bit 0 |
| `wake_at(time)` | asks for an accepted point at `time` |

A pin is given by its name (case-insensitive) or by a `PinIndex`. A name is looked up among the
pins at every call; a model with many pins can look them up once, with `CosimDevice::pin_index(name)`
before the device is put in the circuit or `DeviceIo::pin_index(name)` in `step`:

```rust
let device = CosimDevice::new("sram")
    .input("cs", InputCharacteristics::cmos(3.3))
    /* .input("a0", ...) ... .output("d7", ...) */;
let model = Sram {
    cs: device.pin_index("cs"),
    address: ["a0", "a1", "a2", "a3"].map(|pin| device.pin_index(pin)),
    data: ["d0", "d1", "d2", "d3", "d4", "d5", "d6", "d7"].map(|pin| device.pin_index(pin)),
    memory: [0; 16],
};

impl DeviceModel for Sram {
    fn step(&mut self, io: &mut DeviceIo<'_>) {
        if io.fell(self.cs) {
            let address = io.read_bus(self.address) as usize;
            io.set_bus(self.data, u32::from(self.memory[address]));
        }
    }
}
```

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
  `rose` and `fell` compare the reads of two accepted points, so they report an edge at that
  point too; a pulse shorter than the time between two points can be missed.
- Outputs start `Low` and bidirectional pins `HiZ` until the model sets them. The operating point
  before the transient analysis is computed with these levels. Other analyses (`op`, `ac`, `dc`) see
  the outputs as they are and do not call the models.
- A run with several transient analyses starts each from time 0 with the pins as above. The model
  is not reset: reset its state when `io.is_first_point()` is true. `Periodic` and `Pwm` start
  over by themselves, and `Firmware` of `cuprite_wire_embedded_hal` drives the outputs the firmware
  has set to their levels again (the firmware's own state is kept).
- The models read the voltages of the pins from the results of the analysis. A `save` command that
  leaves out the node of a pin (or of `gnd`) makes `run_with` return
  `SimulationError::Cosimulation`, and the models are not called.
- A panic in a model (e.g. an `assert!` or an unknown pin name) is resumed by `run_with` after the
  run, so it fails the test as usual. The analysis still runs to its end, without calling the
  models: the outputs of all devices are released (high impedance, with the drive source at 0 V).

## 3. Peripherals

`cuprite_wire::spice::cosim::peripherals` has models of common peripherals, called from `step`:

- `Periodic::new(period, start)`: `due(io)` is true once per period and asks for the next time point.
  Use it for timer interrupts and control loops.
- `Adc::new(pin, bits, reference)`: `read(io)` converts the pin voltage
  (`floor(v / reference * 2^bits)`, clamped); `voltage_of(code)` converts back.
- `Pwm::new(pin, period, start)`: `set_duty(duty)` takes effect at the next period;
  call `update(io)` at every step. `update` takes the duty when it starts a period, so call it
  before the code setting the duty: otherwise a duty set at the time a period starts (a control
  loop with the same or a multiple of the PWM period) takes effect in that period.

```rust
impl DeviceModel for Controller {
    fn step(&mut self, io: &mut DeviceIo<'_>) {
        self.pwm.update(io);
        if self.control.due(io) {
            let measured = self.adc.voltage_of(self.adc.read(io));
            self.pwm.set_duty(self.pwm.duty() + 0.05 * (TARGET - measured));
        }
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
  `open_drain_pin(pin)` (`InputPin`, `OutputPin`, `StatefulOutputPin` on a bidirectional pin),
  `pwm(pin, period, max_duty)` (`SetDutyCycle`; a new duty takes effect at the next period, also
  when the task sets it at the time a period starts) and `adc(pin, bits, reference)`
  (`AdcChannel`). The errors are `Infallible`. An output pin stays as the device starts it (`Low`,
  or `HiZ` for a bidirectional pin) until it is set; until then neither `is_set_high()` nor
  `is_set_low()` is true.
- **Open-drain pins** (I2C, wired-OR lines): `set_low()` drives the pin `Low` and `set_high()`
  releases it (`HiZ`), so a pull-up (e.g. `InputCharacteristics::with_pull(Pull::up(..))` of the
  pin, or a part in the circuit) or another device sets the level. `is_high()` reads the line
  whatever drives it, so the firmware sees another device pulling it low. The pin must be
  `bidirectional` in the `CosimDevice`, and starts released (`is_set_high()` is true).
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
.subckt cuprite_cosim_mcu button led ref
CC_button button ref 5e-12
VD_led led#d ref DC 0 EXTERNAL
VE_led led#e ref DC 0 EXTERNAL
SO_led led#d led led#e ref hiz_led
.model hiz_led SW( vt=0.5 vh=0 ron=50 roff=1e9 )
.ends cuprite_cosim_mcu
```

The `gnd` pin is the port `ref` (ngspice reads a node named `gnd` as the ground, even inside a
subcircuit). The internal nodes of a pin are named `<pin>#<role>`, which no pin name can be.
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

## 6. Digital models in shared libraries (XSPICE `d_cosim`)

`cuprite_wire::spice::xspice::DCosim` puts a digital model compiled into a shared library in the
circuit, through ngspice's `d_cosim` code model. It needs no feature and runs with the CLI
(`deck.run()`), so it fits HDL designs and digital models written in C. The library implements
`Cosim_setup` of `cosim.h` (shipped with ngspice in `/usr/share/ngspice/scripts/src/ngspice/`).

This section is supported on Linux only (the Rust models of sections 1 to 5 run on Windows and
macOS too). The libraries are built with the compilers and ngspice's sources of a Linux
installation; on Windows, use WSL 2 or the devcontainer.

```rust
use cuprite_wire::spice::{digital::*, xspice::DCosim};

let counter = DCosim::new("/abs/path/libcounter.so")   // the `simulation` parameter
    .with_sim_args(["8"])                               // `sim_args`, passed to the model
    .input("clk", InputCharacteristics::cmos(3.3))      // input port 0
    .output("q0", OutputCharacteristics::cmos(3.3))     // output port 0
    .output("q1", OutputCharacteristics::cmos(3.3));    // output port 1
let counter = bench.instantiate_part(Named("counter", counter));
```

- The ports of the model are the inputs and the outputs in the order they are added; a model
  without inputs gets `NULL`. Pin names follow the rules of `CosimDevice` (section 1). Bidirectional ports and high impedance are not supported.
- Each input has the load of its `InputCharacteristics` and an `adc_bridge` at its thresholds
  (`in_low` = `threshold_low`, `in_high` = `threshold_high`; between them the model sees the unknown
  state, without hysteresis). Each output has a `dac_bridge` from `low` to `high` with the rise and
  fall times (unknown is the middle), behind the output resistance.
- Voltages are relative to the `gnd` pin: the bridges work against the ground, so they are connected
  through unity-gain sources (`EI_<pin>`, `ED_<pin>`).
- `with_delay(seconds)` sets the delay of the model's outputs (`delay`, 1 ns by default),
  `with_lib_args` the arguments of the library's shim (`lib_args`). Paths must be absolute.
- The model runs at the time points of the analysis and at its input events: a model that changes
  its outputs by itself (an internal clock) does so at the next time point, so give the analysis a
  time step fine enough for it.

### 6.1 Verilog designs (Verilator)

`Verilator` builds Verilog sources into a library for `d_cosim`, as ngspice's `vlnggen` script
does, and reads the ports of the top module to make the part:

```rust
use cuprite_wire::spice::xspice::Verilator;

let model = Verilator::new([concat!(env!("CARGO_MANIFEST_DIR"), "/hdl/counter.v")])
    .arg("--top-module").arg("counter")          // any Verilator argument
    .build()?;                                   // Result<VerilatedModel, VerilatorError>
let counter = model.part(&InputCharacteristics::cmos(3.3), &OutputCharacteristics::cmos(3.3));
// pins: the input bits, the output bits, gnd: e.g. clk, rst, q_1, q_0, carry, gnd
```

- The build runs `verilator --cc` (C++ with the class prefix `Vlng`), reads the ports from the
  generated class, and compiles the design with ngspice's `verilator_shim.cpp` and
  `verilator_main.cpp` (`DEFAULT_SHIM_DIR`, `/usr/share/ngspice/scripts/src`) into a shared library
  with `g++`. It needs `verilator`, `g++` and `make` (the devcontainer has them). A missing
  program or shim is reported as `VerilatorError::Io` that names it (and `shim_dir`, for the shim).
- Builds take a few seconds and are cached by the contents of the sources, the arguments, the
  current directory and the version of Verilator, in `cuprite_wire/verilator` in the user's
  cache directory (`$XDG_CACHE_HOME`, else `~/.cache`, else the system temporary directory;
  `cache_dir` to change it). The libraries built there are loaded into the process, so on Unix
  the cache directory is created with mode `0700`, and one that is writable by the group or
  others, or (on Linux) belongs to another user, is rejected (`VerilatorError::Io`). The files
  Verilator read in besides the sources (`` `include ``, `-y`, `-v`, `-f`) are recorded with their
  size and modification time, and a change to them rebuilds the design.
- Relative paths (the sources, `shim_dir`, `cache_dir` and paths in the arguments) are resolved
  against the current directory.
- A failed step returns `VerilatorError::Command` with the log, and keeps its build directory;
  other errors remove it.
- Ports are taken in the order of their declaration; a port of several bits gives a pin per bit,
  `<name>_<bit>` from the most significant bit. `model.inputs()` / `model.outputs()` list them, to
  build a `DCosim` with other characteristics per pin. Inout ports, ports wider than 32 bits and
  ports whose pins are not valid pin names of the part (`gnd`, `ref`, or a scalar `q_0` next to a
  bus `q`) are not supported (`VerilatorError::Ports`).
- Verilator is cycle-based: the design advances on input changes (the shim uses `After_input`),
  so it needs a clock or other inputs to do anything. Delays (`#10`) need `--timing`, which this
  build does not support.
- Icarus Verilog is not used: the `ivlng.so` shim of Debian's ngspice 44.2 exports no symbols
  (`undefined symbol: Cosim_setup`), and Debian's Icarus Verilog 12 has no `libvvp`.
