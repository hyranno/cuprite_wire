//! [`embedded-hal`](embedded_hal) 1.0 implementations backed by a
//! [`CosimDevice`](cuprite_wire::spice::cosim::CosimDevice), so that the control logic of a
//! firmware, written against the `embedded-hal` traits, runs unchanged against a circuit
//! simulated with ngspice.
//!
//! The firmware is run as a periodic task: [`Firmware`] calls a step function of it every period
//! (as a timer interrupt or the body of a super loop would), and the pins handed to it read the
//! circuit at that time and drive it from then on.
//!
//! ```no_run
//! use cuprite_wire_embedded_hal::{Board, Firmware};
//! use cuprite_wire::spice::cosim::DeviceModels;
//! use embedded_hal::digital::{InputPin, OutputPin};
//!
//! // The firmware's code: generic over the embedded-hal traits.
//! struct Mirror<I, O> { button: I, led: O }
//!
//! impl<I: InputPin, O: OutputPin> Mirror<I, O> {
//!     fn step(&mut self) {
//!         let pressed = self.button.is_low().unwrap();
//!         self.led.set_state(pressed.into()).unwrap();
//!     }
//! }
//!
//! // The test: pins of the `CosimDevice` named "mcu" in the deck.
//! # fn deck() -> cuprite_wire::spice::SpiceDeck { unimplemented!() }
//! let board = Board::new();
//! let mirror = Mirror { button: board.input_pin("button"), led: board.output_pin("led") };
//! let mut firmware = Firmware::new(&board, 10e-6, mirror, Mirror::step);
//! deck().run_with(DeviceModels::new().with("mcu", &mut firmware)).unwrap();
//! ```
//!
//! Blocking waits (`DelayNs`) cannot advance the simulated time from inside a step, so they are
//! not provided: write the waits as states of the periodic task.
//!
//! See the guide [Co-simulation with Rust models](https://github.com/hyranno/cuprite_wire/blob/main/doc/cosimulation.md).

#![warn(missing_docs, clippy::missing_errors_doc, clippy::missing_panics_doc)]

use std::{
    cell::{Ref, RefCell, RefMut},
    convert::Infallible,
    rc::Rc,
};

use cuprite_wire::spice::{
    cosim::{
        DeviceIo, DeviceModel, PinIndex,
        peripherals::{Adc, Periodic, Pwm},
    },
    digital::Level,
};
use embedded_hal::{digital, pwm};

/// An ADC channel: one conversion per call. `embedded-hal` 1.0 has no ADC trait; a firmware built
/// for the target cannot depend on this crate, so it defines an equivalent trait of its own and
/// the test implements it for [`AdcPin`] with this one.
pub trait AdcChannel {
    /// The resolution of the codes.
    fn bits(&self) -> u32;
    /// Converts the voltage of the pin (`floor(v / reference * 2^bits)`, clamped).
    fn read(&mut self) -> u16;
}

/// What the pins of a [`Board`] read and set during a step.
#[derive(Debug, Default)]
struct State {
    inputs: Vec<Input>,
    /// Each ADC: (converter, the code converted at the step).
    adcs: Vec<(Adc, u16)>,
    outputs: Vec<Output>,
    pwms: Vec<Pwm>,
}

/// A pin of the device, looked up by name once per transient analysis.
#[derive(Debug)]
struct Pin {
    name: String,
    index: Option<PinIndex>,
}

impl Pin {
    fn new(name: &str) -> Self {
        Pin {
            name: name.to_string(),
            index: None,
        }
    }

    /// The index of the pin in the device of `io`.
    #[track_caller]
    fn index(&mut self, io: &DeviceIo<'_>) -> PinIndex {
        *self.index.get_or_insert_with(|| io.pin_index(&self.name))
    }
}

/// A digital input of a [`Board`].
#[derive(Debug)]
struct Input {
    pin: Pin,
    /// The level read at the step.
    high: bool,
}

/// A digital output of a [`Board`].
#[derive(Debug)]
struct Output {
    pin: Pin,
    /// The level set last; `None` until it is set.
    level: Option<Level>,
    /// Whether it was set during the step.
    changed: bool,
}

/// The pins of a [`CosimDevice`](cuprite_wire::spice::cosim::CosimDevice) as `embedded-hal`
/// objects. Create the pins, hand them to the firmware, and run the firmware with [`Firmware`].
/// The pins read the circuit and drive it only while [`Firmware`] runs a step.
#[derive(Debug, Clone, Default)]
pub struct Board {
    state: Rc<RefCell<State>>,
}

impl Board {
    /// A board without pins.
    pub fn new() -> Self {
        Self::default()
    }

    /// A digital input (`InputPin`) on a pin with input characteristics.
    pub fn input_pin(&self, pin: &str) -> InputPinHandle {
        let mut state = self.state.borrow_mut();
        state.inputs.push(Input {
            pin: Pin::new(pin),
            high: false,
        });
        InputPinHandle(self.handle(state.inputs.len()))
    }

    /// A digital output (`OutputPin`, `StatefulOutputPin`) on a pin with output characteristics.
    /// Until it is set, the pin stays as the device starts it (`Low` for an output, `HiZ` for a
    /// bidirectional pin), and neither `is_set_high` nor `is_set_low` is true; `toggle` sets it high.
    pub fn output_pin(&self, pin: &str) -> OutputPinHandle {
        let mut state = self.state.borrow_mut();
        state.outputs.push(Output {
            pin: Pin::new(pin),
            level: None,
            changed: false,
        });
        OutputPinHandle(self.handle(state.outputs.len()))
    }

    /// An open-drain pin (`InputPin`, `OutputPin`, `StatefulOutputPin`) on a bidirectional pin, as
    /// for I2C or a wired-OR line: `set_low` drives it `Low` and `set_high` releases it (`HiZ`), so
    /// a pull-up or another device sets its level then. `is_high` reads the pin, whatever drives it.
    /// It starts released (`is_set_high` is true), as the bidirectional pin starts in `HiZ`.
    pub fn open_drain_pin(&self, pin: &str) -> OpenDrainPinHandle {
        let mut state = self.state.borrow_mut();
        state.inputs.push(Input {
            pin: Pin::new(pin),
            high: false,
        });
        let input = state.inputs.len() - 1;
        state.outputs.push(Output {
            pin: Pin::new(pin),
            level: Some(Level::HiZ),
            changed: false,
        });
        OpenDrainPinHandle {
            output: self.handle(state.outputs.len()),
            input,
        }
    }

    /// A PWM channel (`SetDutyCycle`) on an output pin, with `period` seconds and duty steps of
    /// `1 / max_duty`. A new duty takes effect at the next period, also when it is set by a step
    /// at the time a period starts.
    ///
    /// # Panics
    /// If `period` is not positive or `max_duty` is 0.
    #[track_caller]
    pub fn pwm(&self, pin: &str, period: f64, max_duty: u16) -> PwmChannel {
        assert!(max_duty > 0, "the maximum duty cycle must be positive");
        let mut state = self.state.borrow_mut();
        state.pwms.push(Pwm::new(pin, period, 0.0));
        PwmChannel {
            handle: self.handle(state.pwms.len()),
            max_duty,
        }
    }

    /// An ADC channel ([`AdcChannel`]) on a pin, converting to `bits` bits with full scale at
    /// `reference` volts. It converts at the start of each step.
    ///
    /// # Panics
    /// If `bits` is not in 1..=16 or `reference` is not positive.
    #[track_caller]
    pub fn adc(&self, pin: &str, bits: u32, reference: f64) -> AdcPin {
        assert!(
            (1..=16).contains(&bits),
            "the resolution must be 1 to 16 bits: {bits}"
        );
        let mut state = self.state.borrow_mut();
        state.adcs.push((Adc::new(pin, bits, reference), 0));
        AdcPin(self.handle(state.adcs.len()))
    }

    /// A handle to the last of `len` pins of a kind.
    fn handle(&self, len: usize) -> Handle {
        Handle {
            state: self.state.clone(),
            index: len - 1,
        }
    }

    /// Reads the pins for a step.
    fn capture(&self, io: &DeviceIo<'_>) {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        for input in &mut state.inputs {
            input.high = io.read(input.pin.index(io));
        }
        for (adc, code) in &mut state.adcs {
            *code = adc.read(io) as u16;
        }
    }

    /// At the first point of a transient analysis, the pins of the device start over (`Low` or
    /// `HiZ`): drives the outputs that have been set to their levels again. The pins are looked up
    /// again in the device of the analysis.
    fn restart(&self) {
        let mut state = self.state.borrow_mut();
        for input in &mut state.inputs {
            input.pin.index = None;
        }
        for output in &mut state.outputs {
            output.pin.index = None;
            output.changed |= output.level.is_some();
        }
    }

    /// Drives the PWM channels, before the step: a period starting now takes the duty set before.
    fn update_pwms(&self, io: &mut DeviceIo<'_>) {
        for pwm in &mut self.state.borrow_mut().pwms {
            pwm.update(io);
        }
    }

    /// Drives the outputs set during the step.
    fn apply(&self, io: &mut DeviceIo<'_>) {
        for output in &mut self.state.borrow_mut().outputs {
            if let (true, Some(level)) = (std::mem::take(&mut output.changed), output.level) {
                let pin = output.pin.index(io);
                io.set(pin, level);
            }
        }
    }
}

/// A firmware running as a periodic task: a [`DeviceModel`] calling `step(&mut task)` every
/// `period` seconds from time 0, with the pins of `board`.
///
/// A run with several transient analyses runs the firmware from time 0 in each, without resetting
/// `task`: the firmware goes on as if the time had jumped back, and the outputs it has set are
/// driven to their levels again.
pub struct Firmware<T, F> {
    board: Board,
    timer: Periodic,
    task: T,
    step: F,
}

impl<T, F: FnMut(&mut T)> Firmware<T, F> {
    /// # Panics
    /// If `period` is not positive.
    #[track_caller]
    pub fn new(board: &Board, period: f64, task: T, step: F) -> Self {
        Firmware {
            board: board.clone(),
            timer: Periodic::new(period, 0.0),
            task,
            step,
        }
    }

    /// The firmware's state, e.g. to check it after the run.
    pub fn task(&self) -> &T {
        &self.task
    }

    /// The firmware's state, consuming the firmware.
    pub fn into_task(self) -> T {
        self.task
    }
}

impl<T, F: FnMut(&mut T)> DeviceModel for Firmware<T, F> {
    fn step(&mut self, io: &mut DeviceIo<'_>) {
        if io.is_first_point() {
            self.board.restart();
        }
        self.board.update_pwms(io);
        if self.timer.due(io) {
            self.board.capture(io);
            (self.step)(&mut self.task);
        }
        self.board.apply(io);
    }
}

/// A pin of a kind (the index of its entry in the [`State`] of a [`Board`]).
#[derive(Debug)]
struct Handle {
    state: Rc<RefCell<State>>,
    index: usize,
}

impl Handle {
    fn state(&self) -> Ref<'_, State> {
        self.state.borrow()
    }

    fn state_mut(&self) -> RefMut<'_, State> {
        self.state.borrow_mut()
    }

    /// Sets the output of this index to `level`.
    fn set_output(&self, level: Level) {
        let mut state = self.state_mut();
        let output = &mut state.outputs[self.index];
        output.level = Some(level);
        output.changed = true;
    }

    /// The level the output of this index was set to last.
    fn output_level(&self) -> Option<Level> {
        self.state().outputs[self.index].level
    }

    /// The level the input of index `input` read at the step.
    fn input_high(&self, input: usize) -> bool {
        self.state().inputs[input].high
    }
}

/// A digital input of a [`Board`].
#[derive(Debug)]
pub struct InputPinHandle(Handle);

impl digital::ErrorType for InputPinHandle {
    type Error = Infallible;
}

impl digital::InputPin for InputPinHandle {
    fn is_high(&mut self) -> Result<bool, Infallible> {
        Ok(self.0.input_high(self.0.index))
    }

    fn is_low(&mut self) -> Result<bool, Infallible> {
        self.is_high().map(|high| !high)
    }
}

/// A digital output of a [`Board`].
#[derive(Debug)]
pub struct OutputPinHandle(Handle);

impl digital::ErrorType for OutputPinHandle {
    type Error = Infallible;
}

impl digital::OutputPin for OutputPinHandle {
    fn set_low(&mut self) -> Result<(), Infallible> {
        self.0.set_output(Level::Low);
        Ok(())
    }

    fn set_high(&mut self) -> Result<(), Infallible> {
        self.0.set_output(Level::High);
        Ok(())
    }
}

impl digital::StatefulOutputPin for OutputPinHandle {
    fn is_set_high(&mut self) -> Result<bool, Infallible> {
        Ok(self.0.output_level() == Some(Level::High))
    }

    fn is_set_low(&mut self) -> Result<bool, Infallible> {
        Ok(self.0.output_level() == Some(Level::Low))
    }

    /// Sets the output low if it is set high, and high otherwise: an output not yet set starts
    /// `Low` (or `HiZ`), so its first toggle drives it high. (The default of embedded-hal would
    /// set it low, since it is not set low either.)
    fn toggle(&mut self) -> Result<(), Infallible> {
        let level = if self.0.output_level() == Some(Level::High) {
            Level::Low
        } else {
            Level::High
        };
        self.0.set_output(level);
        Ok(())
    }
}

/// An open-drain pin of a [`Board`]: `High` releases it.
#[derive(Debug)]
pub struct OpenDrainPinHandle {
    /// The output entry.
    output: Handle,
    /// The index of the input entry.
    input: usize,
}

impl digital::ErrorType for OpenDrainPinHandle {
    type Error = Infallible;
}

impl digital::InputPin for OpenDrainPinHandle {
    fn is_high(&mut self) -> Result<bool, Infallible> {
        Ok(self.output.input_high(self.input))
    }

    fn is_low(&mut self) -> Result<bool, Infallible> {
        self.is_high().map(|high| !high)
    }
}

impl digital::OutputPin for OpenDrainPinHandle {
    fn set_low(&mut self) -> Result<(), Infallible> {
        self.output.set_output(Level::Low);
        Ok(())
    }

    fn set_high(&mut self) -> Result<(), Infallible> {
        self.output.set_output(Level::HiZ);
        Ok(())
    }
}

impl digital::StatefulOutputPin for OpenDrainPinHandle {
    fn is_set_high(&mut self) -> Result<bool, Infallible> {
        Ok(self.output.output_level() == Some(Level::HiZ))
    }

    fn is_set_low(&mut self) -> Result<bool, Infallible> {
        Ok(self.output.output_level() == Some(Level::Low))
    }
}

/// A PWM channel of a [`Board`].
#[derive(Debug)]
pub struct PwmChannel {
    handle: Handle,
    max_duty: u16,
}

impl pwm::ErrorType for PwmChannel {
    type Error = Infallible;
}

impl pwm::SetDutyCycle for PwmChannel {
    fn max_duty_cycle(&self) -> u16 {
        self.max_duty
    }

    fn set_duty_cycle(&mut self, duty: u16) -> Result<(), Infallible> {
        let duty = f64::from(duty.min(self.max_duty)) / f64::from(self.max_duty);
        self.handle.state_mut().pwms[self.handle.index].set_duty(duty);
        Ok(())
    }
}

/// An ADC channel of a [`Board`].
#[derive(Debug)]
pub struct AdcPin(Handle);

impl AdcChannel for AdcPin {
    fn bits(&self) -> u32 {
        self.0.state().adcs[self.0.index].0.bits()
    }

    fn read(&mut self) -> u16 {
        self.0.state().adcs[self.0.index].1
    }
}
