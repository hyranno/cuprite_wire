//! Co-simulation with Rust models of devices, with the `shared-ngspice` feature: a model of a
//! microcontroller (its firmware's control logic) reads the voltages of its pins and drives them
//! while ngspice simulates the circuit.
//!
//! - [`CosimDevice`] is the simulation-only part standing for the device in the test bench. Each
//!   pin has the electrical model of an output ([`OutputCharacteristics`]), of an input
//!   ([`InputCharacteristics`]) or both; outputs are driven by `EXTERNAL` sources that ngspice asks
//!   the model for.
//! - [`DeviceModel`] is the model: [`DeviceModel::step`] is called at every accepted time point of
//!   the transient analysis with a [`DeviceIo`], which reads the pins and changes the outputs.
//! - [`SpiceDeck::run_with`] runs a deck with the models bound to the devices by name
//!   ([`DeviceModels`]), in the ngspice shared library ([`SharedNgspice`]).
//!
//! [`peripherals`] has models of common peripherals (ADC, PWM, periodic tasks) to build models with.
//!
//! [`OutputCharacteristics`]: crate::spice::digital::OutputCharacteristics
//! [`InputCharacteristics`]: crate::spice::digital::InputCharacteristics
//! [`SharedNgspice`]: crate::spice::SharedNgspice

mod device;
pub mod peripherals;
mod session;

pub use device::{CosimDevice, CosimPin};
pub(crate) use session::CosimSession;

use crate::spice::{
    SharedNgspice, SpiceDeck,
    digital::Level,
    simulation::{Simulation, SimulationError},
};

/// The model of a device: the behavior behind the pins of a [`CosimDevice`].
pub trait DeviceModel {
    /// Called at every accepted time point of a transient analysis, from time 0 on, after the
    /// voltages of the pins have been read. Changes made through `io` take effect from this time
    /// point on; the state of the model should only change here.
    ///
    /// Between two calls the outputs keep what was set, so a change between accepted points
    /// needs a time point at that time: ask for one with [`DeviceIo::wake_at`].
    fn step(&mut self, io: &mut DeviceIo<'_>);
}

/// What an output pin is told to do.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drive {
    Level(Level),
    Voltage(f64),
}

/// The state of a pin during a run.
#[derive(Debug, Clone)]
pub(crate) struct PinState {
    pin: CosimPin,
    /// The voltage at the last accepted point, relative to the `gnd` pin of the device.
    voltage: f64,
    /// The level read at the last accepted point (inputs only).
    read: bool,
    /// Set by the model during [`DeviceModel::step`].
    request: Option<Drive>,
}

/// The pins of a device during [`DeviceModel::step`].
///
/// The methods panic for a pin the device does not have, and the output methods for a pin
/// without [`OutputCharacteristics`]. After a panic in a model, the run goes on without calling the
/// models, and `run_with` resumes the panic when it ends.
///
/// [`OutputCharacteristics`]: crate::spice::digital::OutputCharacteristics
pub struct DeviceIo<'a> {
    time: f64,
    pins: &'a mut [PinState],
    wakes: &'a mut Vec<f64>,
}

impl DeviceIo<'_> {
    /// The time of the accepted point, in seconds.
    pub fn time(&self) -> f64 {
        self.time
    }

    #[track_caller]
    fn pin(&self, name: &str) -> &PinState {
        let index = self.index(name);
        &self.pins[index]
    }

    #[track_caller]
    fn index(&self, name: &str) -> usize {
        self.pins
            .iter()
            .position(|pin| pin.pin.name.eq_ignore_ascii_case(name))
            .unwrap_or_else(|| {
                let names: Vec<&str> = self.pins.iter().map(|p| p.pin.name.as_str()).collect();
                panic!("the device has no pin {name:?} (pins: {names:?})")
            })
    }

    /// The voltage of the pin relative to the `gnd` pin of the device.
    #[track_caller]
    pub fn voltage(&self, pin: &str) -> f64 {
        self.pin(pin).voltage
    }

    /// The level the input reads, with the thresholds and hysteresis of its
    /// [`InputCharacteristics`](crate::spice::digital::InputCharacteristics).
    ///
    /// # Panics
    /// If the pin has no input characteristics.
    #[track_caller]
    pub fn read(&self, pin: &str) -> bool {
        let state = self.pin(pin);
        assert!(state.pin.input.is_some(), "the pin {pin:?} is not an input");
        state.read
    }

    /// Drives the output to a level, with the edges of its
    /// [`OutputCharacteristics`](crate::spice::digital::OutputCharacteristics).
    #[track_caller]
    pub fn set(&mut self, pin: &str, level: Level) {
        self.request(pin, Drive::Level(level));
    }

    /// Drives the output to a voltage between its `low` and `high` levels (e.g. a DAC), changing
    /// at the rate of its edges.
    #[track_caller]
    pub fn set_voltage(&mut self, pin: &str, voltage: f64) {
        self.request(pin, Drive::Voltage(voltage));
    }

    #[track_caller]
    fn request(&mut self, pin: &str, drive: Drive) {
        let index = self.index(pin);
        let state = &mut self.pins[index];
        assert!(
            state.pin.output.is_some(),
            "the pin {pin:?} is not an output"
        );
        state.request = Some(drive);
    }

    /// Asks for an accepted time point at `time` (a breakpoint of ngspice), so that the model is
    /// called then. Times not after the current one are ignored.
    pub fn wake_at(&mut self, time: f64) {
        if time > self.time {
            self.wakes.push(time);
        }
    }
}

/// The models of a run, bound to [`CosimDevice`]s by name.
#[derive(Default)]
pub struct DeviceModels<'a> {
    models: Vec<(String, &'a mut dyn DeviceModel)>,
}

impl<'a> DeviceModels<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds `model` to the device named `name`.
    pub fn with(mut self, name: impl Into<String>, model: &'a mut dyn DeviceModel) -> Self {
        self.models.push((name.into(), model));
        self
    }
}

impl SharedNgspice {
    /// Runs the deck with the models of its [`CosimDevice`]s. Every device needs a model and
    /// every model a device.
    ///
    /// # Panics
    /// Resumes a panic of a model after the run.
    pub fn run_with(
        &self,
        deck: &SpiceDeck,
        models: DeviceModels<'_>,
    ) -> Result<Simulation, SimulationError> {
        let mut session = CosimSession::new(deck.netlist().cosim_devices(), models.models)
            .map_err(SimulationError::Cosimulation)?;
        self.run_session(deck, Some(&mut session))
    }
}

impl SpiceDeck {
    /// Runs the deck with the models of its [`CosimDevice`]s in the ngspice shared library
    /// ([`SharedNgspice::default`]); see [`SharedNgspice::run_with`].
    pub fn run_with(&self, models: DeviceModels<'_>) -> Result<Simulation, SimulationError> {
        SharedNgspice::default().run_with(self, models)
    }
}

/// A [`CosimDevice`] in a netlist: its element (`XU1`) and the nodes of its pins, in the order of
/// [`CosimDevice::pins`] followed by `gnd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CosimInstance {
    pub(crate) element: String,
    pub(crate) nodes: Vec<String>,
    pub(crate) device: CosimDevice,
}

#[cfg(test)]
mod tests;
