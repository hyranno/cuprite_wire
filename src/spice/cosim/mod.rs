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

pub use device::{CosimDevice, CosimPin, PinIndex};
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
    /// The level read at the accepted point before (the same as `read` at the first point).
    previous: bool,
    /// Set by the model during [`DeviceModel::step`].
    request: Option<Drive>,
}

impl PinState {
    pub(crate) fn new(pin: CosimPin) -> Self {
        PinState {
            pin,
            voltage: 0.0,
            read: false,
            previous: false,
            request: None,
        }
    }
}

/// A pin given to the methods of [`DeviceIo`]: its name (`&str`, `String`, anything
/// `AsRef<str>`), case-insensitive, or its [`PinIndex`].
pub trait PinRef: sealed::Sealed {}

impl<T: sealed::Sealed> PinRef for T {}

mod sealed {
    use super::{DeviceIo, PinIndex};

    pub trait Sealed {
        /// The index of the pin in the pins of the device.
        #[track_caller]
        fn index_in(&self, io: &DeviceIo<'_>) -> usize;
    }

    impl<T: AsRef<str>> Sealed for T {
        #[track_caller]
        fn index_in(&self, io: &DeviceIo<'_>) -> usize {
            io.index_of(self.as_ref())
        }
    }

    impl Sealed for PinIndex {
        #[track_caller]
        fn index_in(&self, io: &DeviceIo<'_>) -> usize {
            let count = io.pins.len();
            assert!(
                self.0 < count,
                "the device has no pin {self:?} (it has {count} pins)"
            );
            self.0
        }
    }

    impl Sealed for &PinIndex {
        #[track_caller]
        fn index_in(&self, io: &DeviceIo<'_>) -> usize {
            (*self).index_in(io)
        }
    }
}

/// The pins of a device during [`DeviceModel::step`].
///
/// Pins are given by name or by [`PinIndex`] ([`PinRef`]). A name is looked up among the pins at
/// every call; a model with many pins can look them up once with [`CosimDevice::pin_index`] or
/// [`DeviceIo::pin_index`].
///
/// The methods panic for a pin the device does not have, the input methods for a pin without
/// [`InputCharacteristics`] and the output methods for a pin without [`OutputCharacteristics`].
/// After a panic in a model, the run goes on without calling the models, and `run_with` resumes the
/// panic when it ends.
///
/// [`InputCharacteristics`]: crate::spice::digital::InputCharacteristics
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

    /// The index of the pin named `name` (case-insensitive), as [`CosimDevice::pin_index`].
    ///
    /// # Panics
    /// If the device has no such pin.
    #[track_caller]
    pub fn pin_index(&self, name: &str) -> PinIndex {
        PinIndex(self.index_of(name))
    }

    #[track_caller]
    fn index_of(&self, name: &str) -> usize {
        device::find_pin(self.pins.iter().map(|state| &state.pin), name).unwrap_or_else(|| {
            let names: Vec<&str> = self.pins.iter().map(|p| p.pin.name.as_str()).collect();
            panic!("the device has no pin {name:?} (pins: {names:?})")
        })
    }

    #[track_caller]
    fn state(&self, pin: impl PinRef) -> &PinState {
        &self.pins[pin.index_in(self)]
    }

    #[track_caller]
    fn input(&self, pin: impl PinRef) -> &PinState {
        let state = self.state(pin);
        assert!(
            state.pin.input.is_some(),
            "the pin {:?} is not an input",
            state.pin.name
        );
        state
    }

    /// The voltage of the pin relative to the `gnd` pin of the device.
    #[track_caller]
    pub fn voltage(&self, pin: impl PinRef) -> f64 {
        self.state(pin).voltage
    }

    /// The level the input reads, with the thresholds and hysteresis of its
    /// [`InputCharacteristics`](crate::spice::digital::InputCharacteristics).
    ///
    /// # Panics
    /// If the pin has no input characteristics.
    #[track_caller]
    pub fn read(&self, pin: impl PinRef) -> bool {
        self.input(pin).read
    }

    /// Whether the input reads high now and read low at the accepted point before. Always false
    /// at the first point of the analysis.
    ///
    /// # Panics
    /// As [`DeviceIo::read`].
    #[track_caller]
    pub fn rose(&self, pin: impl PinRef) -> bool {
        let state = self.input(pin);
        state.read && !state.previous
    }

    /// Whether the input reads low now and read high at the accepted point before. Always false
    /// at the first point of the analysis.
    ///
    /// # Panics
    /// As [`DeviceIo::read`].
    #[track_caller]
    pub fn fell(&self, pin: impl PinRef) -> bool {
        let state = self.input(pin);
        !state.read && state.previous
    }

    /// The levels the inputs read as a number: the first pin is bit 0 (the least significant).
    ///
    /// # Panics
    /// If there are more than 32 pins, or as [`DeviceIo::read`].
    #[track_caller]
    pub fn read_bus<P: PinRef>(&self, pins: impl IntoIterator<Item = P>) -> u32 {
        let mut value = 0;
        for (bit, pin) in pins.into_iter().enumerate() {
            assert!(bit < 32, "a bus has at most 32 pins");
            value |= u32::from(self.read(pin)) << bit;
        }
        value
    }

    /// Drives the outputs to the bits of `value`: the first pin to bit 0 (the least significant),
    /// `High` for 1 and `Low` for 0.
    ///
    /// # Panics
    /// If there are more than 32 pins, `value` does not fit in them, or as [`DeviceIo::set`].
    #[track_caller]
    pub fn set_bus<P: PinRef>(&mut self, pins: impl IntoIterator<Item = P>, value: u32) {
        let indices: Vec<usize> = pins.into_iter().map(|pin| pin.index_in(self)).collect();
        let count = indices.len();
        assert!(count <= 32, "a bus has at most 32 pins: {count}");
        assert!(
            count == 32 || value >> count == 0,
            "the value {value:#x} does not fit in {count} pins"
        );
        for (bit, index) in indices.into_iter().enumerate() {
            self.set(PinIndex(index), Level::from(value >> bit & 1 == 1));
        }
    }

    /// Drives the output to a level, with the edges of its
    /// [`OutputCharacteristics`](crate::spice::digital::OutputCharacteristics).
    #[track_caller]
    pub fn set(&mut self, pin: impl PinRef, level: Level) {
        self.request(pin, Drive::Level(level));
    }

    /// Drives the output to a voltage between its `low` and `high` levels (e.g. a DAC), changing
    /// at the rate of its edges.
    #[track_caller]
    pub fn set_voltage(&mut self, pin: impl PinRef, voltage: f64) {
        self.request(pin, Drive::Voltage(voltage));
    }

    #[track_caller]
    fn request(&mut self, pin: impl PinRef, drive: Drive) {
        let index = pin.index_in(self);
        let state = &mut self.pins[index];
        assert!(
            state.pin.output.is_some(),
            "the pin {:?} is not an output",
            state.pin.name
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
