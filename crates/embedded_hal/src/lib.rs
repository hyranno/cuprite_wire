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

use std::{cell::RefCell, convert::Infallible, rc::Rc};

use cuprite_wire::spice::{
    cosim::{
        DeviceIo, DeviceModel,
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
    /// Each input: (pin name, the level read at the step).
    inputs: Vec<(String, bool)>,
    /// Each ADC: (converter, the code converted at the step).
    adcs: Vec<(Adc, u16)>,
    /// Each output: (pin name, the level set, whether it was set during the step).
    outputs: Vec<(String, Level, bool)>,
    pwms: Vec<Pwm>,
}

/// The pins of a [`CosimDevice`](cuprite_wire::spice::cosim::CosimDevice) as `embedded-hal`
/// objects. Create the pins, hand them to the firmware, and run the firmware with [`Firmware`].
/// The pins read the circuit and drive it only while [`Firmware`] runs a step.
#[derive(Debug, Clone, Default)]
pub struct Board {
    state: Rc<RefCell<State>>,
}

impl Board {
    pub fn new() -> Self {
        Self::default()
    }

    /// A digital input (`InputPin`) on a pin with input characteristics.
    pub fn input_pin(&self, pin: &str) -> InputPinHandle {
        let mut state = self.state.borrow_mut();
        state.inputs.push((pin.to_string(), false));
        InputPinHandle {
            state: self.state.clone(),
            index: state.inputs.len() - 1,
        }
    }

    /// A digital output (`OutputPin`, `StatefulOutputPin`) on a pin with output characteristics.
    /// It drives `Low` until it is set.
    pub fn output_pin(&self, pin: &str) -> OutputPinHandle {
        let mut state = self.state.borrow_mut();
        state.outputs.push((pin.to_string(), Level::Low, false));
        OutputPinHandle {
            state: self.state.clone(),
            index: state.outputs.len() - 1,
        }
    }

    /// A PWM channel (`SetDutyCycle`) on an output pin, with `period` seconds and duty steps of
    /// `1 / max_duty`. A new duty takes effect at the next period.
    ///
    /// # Panics
    /// If `period` is not positive or `max_duty` is 0.
    #[track_caller]
    pub fn pwm(&self, pin: &str, period: f64, max_duty: u16) -> PwmChannel {
        assert!(max_duty > 0, "the maximum duty cycle must be positive");
        let mut state = self.state.borrow_mut();
        state.pwms.push(Pwm::new(pin, period, 0.0));
        PwmChannel {
            state: self.state.clone(),
            index: state.pwms.len() - 1,
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
        AdcPin {
            state: self.state.clone(),
            index: state.adcs.len() - 1,
        }
    }

    /// Reads the pins for a step.
    fn capture(&self, io: &DeviceIo<'_>) {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        for (pin, read) in &mut state.inputs {
            *read = io.read(pin);
        }
        for (adc, code) in &mut state.adcs {
            *code = adc.read(io) as u16;
        }
    }

    /// Drives the outputs set during the step, and the PWM channels.
    fn apply(&self, io: &mut DeviceIo<'_>) {
        let mut state = self.state.borrow_mut();
        for (pin, level, changed) in &mut state.outputs {
            if std::mem::take(changed) {
                io.set(pin, *level);
            }
        }
        for pwm in &mut state.pwms {
            pwm.update(io);
        }
    }
}

/// A firmware running as a periodic task: a [`DeviceModel`] calling `step(&mut task)` every
/// `period` seconds from time 0, with the pins of `board`.
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

    pub fn into_task(self) -> T {
        self.task
    }
}

impl<T, F: FnMut(&mut T)> DeviceModel for Firmware<T, F> {
    fn step(&mut self, io: &mut DeviceIo<'_>) {
        if self.timer.due(io) {
            self.board.capture(io);
            (self.step)(&mut self.task);
        }
        self.board.apply(io);
    }
}

/// A digital input of a [`Board`].
#[derive(Debug)]
pub struct InputPinHandle {
    state: Rc<RefCell<State>>,
    index: usize,
}

impl digital::ErrorType for InputPinHandle {
    type Error = Infallible;
}

impl digital::InputPin for InputPinHandle {
    fn is_high(&mut self) -> Result<bool, Infallible> {
        Ok(self.state.borrow().inputs[self.index].1)
    }

    fn is_low(&mut self) -> Result<bool, Infallible> {
        self.is_high().map(|high| !high)
    }
}

/// A digital output of a [`Board`].
#[derive(Debug)]
pub struct OutputPinHandle {
    state: Rc<RefCell<State>>,
    index: usize,
}

impl OutputPinHandle {
    fn set(&mut self, level: Level) {
        let mut state = self.state.borrow_mut();
        let output = &mut state.outputs[self.index];
        output.1 = level;
        output.2 = true;
    }
}

impl digital::ErrorType for OutputPinHandle {
    type Error = Infallible;
}

impl digital::OutputPin for OutputPinHandle {
    fn set_low(&mut self) -> Result<(), Infallible> {
        self.set(Level::Low);
        Ok(())
    }

    fn set_high(&mut self) -> Result<(), Infallible> {
        self.set(Level::High);
        Ok(())
    }
}

impl digital::StatefulOutputPin for OutputPinHandle {
    fn is_set_high(&mut self) -> Result<bool, Infallible> {
        Ok(self.state.borrow().outputs[self.index].1 == Level::High)
    }

    fn is_set_low(&mut self) -> Result<bool, Infallible> {
        self.is_set_high().map(|high| !high)
    }
}

/// A PWM channel of a [`Board`].
#[derive(Debug)]
pub struct PwmChannel {
    state: Rc<RefCell<State>>,
    index: usize,
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
        self.state.borrow_mut().pwms[self.index].set_duty(duty);
        Ok(())
    }
}

/// An ADC channel of a [`Board`].
#[derive(Debug)]
pub struct AdcPin {
    state: Rc<RefCell<State>>,
    index: usize,
}

impl AdcChannel for AdcPin {
    fn bits(&self) -> u32 {
        self.state.borrow().adcs[self.index].0.bits
    }

    fn read(&mut self) -> u16 {
        self.state.borrow().adcs[self.index].1
    }
}
