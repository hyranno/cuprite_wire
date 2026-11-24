# cuprite_wire_embedded_hal

[`embedded-hal`](https://crates.io/crates/embedded-hal) 1.0 implementations backed by a
co-simulated device of [cuprite_wire](https://crates.io/crates/cuprite_wire), so that the control
logic of a firmware, written against the `embedded-hal` traits, runs unchanged against a circuit
simulated with ngspice.

```rust,no_run
use cuprite_wire_embedded_hal::{Board, Firmware};
use cuprite_wire::spice::cosim::DeviceModels;
use embedded_hal::digital::{InputPin, OutputPin};

// The firmware's code: generic over the embedded-hal traits.
struct Mirror<I, O> { button: I, led: O }

impl<I: InputPin, O: OutputPin> Mirror<I, O> {
    fn step(&mut self) {
        let pressed = self.button.is_low().unwrap();
        self.led.set_state(pressed.into()).unwrap();
    }
}

// The test: pins of the `CosimDevice` named "mcu" in `deck`, the `SpiceDeck` of the test bench.
let board = Board::new();
let mirror = Mirror { button: board.input_pin("button"), led: board.output_pin("led") };
let mut firmware = Firmware::new(&board, 10e-6, mirror, Mirror::step);
deck.run_with(DeviceModels::new().with("mcu", &mut firmware)).unwrap();
```

The firmware runs as a periodic task: `Firmware` calls its step function every period, and the
pins read the circuit at that time and drive it from then on. Digital input, output and
open-drain pins, PWM and a minimal ADC channel trait are provided.

## Requirements

The ngspice shared library (`libngspice0` on Debian and Ubuntu), loaded at run time. See the
[requirements of cuprite_wire](https://github.com/hyranno/cuprite_wire#requirements).

## Documentation

- [API documentation](https://docs.rs/cuprite_wire_embedded_hal)
- [Co-simulation with Rust models](https://github.com/hyranno/cuprite_wire/blob/main/doc/cosimulation.md)

## License

MIT
