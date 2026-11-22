//! Simulation-only parts of IBIS models (feature `ibis`): a driver that plays a
//! [`DigitalSequence`] through the I-V tables and ramps of an output buffer, and the receiver of
//! an input buffer.
//!
//! An [`IbisBuffer`] is a `[Model]` of an IBIS file ([`crate::ibis`]) with the package
//! parasitics of a pin, at a [`Corner`]. [`IbisDriver`] and [`IbisReceiver`] put it in a test
//! bench, like [`DigitalDriver`](super::digital::DigitalDriver) and
//! [`DigitalLoad`](super::digital::DigitalLoad) with characteristics taken from a datasheet.
//! The buffer's `Vinl` and `Vinh` read the levels back from the simulated voltage
//! ([`IbisBuffer::input`]).
//!
//! The subcircuits use the XSPICE code model `pwl` (`analog.cm`), which the usual builds of
//! ngspice load.
//!
//! ```no_run
//! use cuprite_wire::{
//!     ibis::Corner,
//!     spice::{
//!         digital::{DigitalSequence, Level},
//!         ibis::{IbisBuffer, IbisDriver, IbisReceiver},
//!     },
//! };
//!
//! let ibs = cuprite_wire::ibis::parse(&std::fs::read_to_string("models/mcu.ibs")?)?;
//! let tx = IbisBuffer::of_pin(&ibs, "MCU", "12", Corner::Typ)?;
//! let rx = IbisBuffer::of_pin(&ibs, "MCU", "13", Corner::Typ)?;
//! let driver = IbisDriver::new(tx, DigitalSequence::new(Level::High).uart(1e-6, 1e6, b"Hi"));
//! let receiver = IbisReceiver::new(rx.clone());
//! // ... connect them, simulate, and read the line with `rx.input().unwrap().decode(...)`.
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;

use cuprite_wire_ibis::{
    Corner, IbisFile, Model, ModelType, PinModel,
    spice::{Drive, Edges, Parasitics, SubcircuitError, subcircuit},
};

use crate::{
    parts::Part,
    spice::{
        ModelSource, SpiceElement, SpicePartInfo, Waveform,
        digital::{DigitalSequence, InputCharacteristics, Level},
        subcircuit::{SUBCIRCUIT_REFERENCE, fnv1a, hashed_subcircuit},
    },
};

/// The reason [`IbisBuffer::of_pin`] has no buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IbisBufferError {
    /// The file has no `[Component]` of the name.
    NoComponent(String),
    /// The component has no pin of the name.
    NoPin {
        /// The component.
        component: String,
        /// The pin.
        pin: String,
    },
    /// The pin is a supply, ground or unconnected pin, names a model the file does not have, or
    /// the model asked for is not one of its `[Model Selector]`.
    NoModel {
        /// The pin.
        pin: String,
        /// What the pin has instead.
        reason: String,
    },
    /// The model cannot be simulated.
    Subcircuit(SubcircuitError),
}

impl fmt::Display for IbisBufferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IbisBufferError::NoComponent(name) => write!(f, "no [Component] `{name}`"),
            IbisBufferError::NoPin { component, pin } => {
                write!(f, "component `{component}` has no pin `{pin}`")
            }
            IbisBufferError::NoModel { pin, reason } => {
                write!(f, "pin `{pin}` has no model: {reason}")
            }
            IbisBufferError::Subcircuit(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for IbisBufferError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IbisBufferError::Subcircuit(error) => Some(error),
            _ => None,
        }
    }
}

impl From<SubcircuitError> for IbisBufferError {
    fn from(error: SubcircuitError) -> Self {
        IbisBufferError::Subcircuit(error)
    }
}

/// An IBIS `[Model]` with the package parasitics of a pin, at a corner, ready to simulate.
#[derive(Debug, Clone, PartialEq)]
pub struct IbisBuffer {
    model_name: String,
    model_type: ModelType,
    corner: Corner,
    /// The name of the subcircuit of the model, `cuprite_ibis_model_<hash>`.
    name: String,
    /// Its definition (ports `pin ref`, and `ku kd` for a driver).
    text: String,
    edges: Option<Edges>,
    notes: Vec<String>,
    input: Option<InputCharacteristics>,
}

impl IbisBuffer {
    /// The buffer of `model` behind `parasitics`, with the values of `corner`.
    ///
    /// # Errors
    /// If the model cannot be written as a subcircuit (see
    /// [`cuprite_wire_ibis::spice::subcircuit`]): its type is not supported or it lacks a table
    /// or the waveforms or `[Ramp]` it needs.
    pub fn new(
        model: &Model,
        parasitics: &Parasitics,
        corner: Corner,
    ) -> Result<Self, SubcircuitError> {
        // The name is derived from the definition, so equal buffers share one definition.
        let hash = fnv1a(subcircuit("", model, parasitics, corner)?.text.as_bytes());
        let name = format!("cuprite_ibis_model_{hash:016x}");
        let subckt = subcircuit(&name, model, parasitics, corner)?;
        let input = model
            .thresholds(corner)
            .map(|(low, high)| InputCharacteristics {
                threshold_low: low,
                threshold_high: high,
                capacitance: model.c_comp.map_or(0.0, |c| c.get(corner)),
                pull: None,
                leakage: 0.0,
            });
        Ok(IbisBuffer {
            model_name: model.name.clone(),
            model_type: model.model_type,
            corner,
            name,
            text: subckt.text,
            edges: subckt.edges,
            notes: subckt.notes,
            input,
        })
    }

    /// The buffer of the pin `pin` of the component `component` of `file`: its model, behind
    /// the parasitics of the pin ([`Parasitics::of_pin`]). For a pin naming a `[Model Selector]`,
    /// the default model of the selector (the first one); [`IbisBuffer::of_pin_model`] chooses
    /// another.
    ///
    /// # Errors
    /// If the file has no such component or pin, the pin has no model of the file (a supply or
    /// `NC` pin), or [`IbisBuffer::new`] fails.
    pub fn of_pin(
        file: &IbisFile,
        component: &str,
        pin: &str,
        corner: Corner,
    ) -> Result<Self, IbisBufferError> {
        Self::pin_buffer(file, component, pin, None, corner)
    }

    /// The buffer of the pin `pin` of the component `component` of `file`, as
    /// [`IbisBuffer::of_pin`] with the model `model` of the `[Model Selector]` of the pin.
    ///
    /// # Errors
    /// As [`IbisBuffer::of_pin`], and if `model` is not one of the models of the selector (or,
    /// for a pin naming a model, not that model).
    pub fn of_pin_model(
        file: &IbisFile,
        component: &str,
        pin: &str,
        model: &str,
        corner: Corner,
    ) -> Result<Self, IbisBufferError> {
        Self::pin_buffer(file, component, pin, Some(model), corner)
    }

    fn pin_buffer(
        file: &IbisFile,
        component: &str,
        pin: &str,
        choice: Option<&str>,
        corner: Corner,
    ) -> Result<Self, IbisBufferError> {
        let c = file
            .component(component)
            .ok_or_else(|| IbisBufferError::NoComponent(component.to_string()))?;
        let p = c.pin(pin).ok_or_else(|| IbisBufferError::NoPin {
            component: component.to_string(),
            pin: pin.to_string(),
        })?;
        let no_model = |reason: String| IbisBufferError::NoModel {
            pin: pin.to_string(),
            reason,
        };
        let model = match &p.model {
            PinModel::Power => return Err(no_model("it is a POWER pin".into())),
            PinModel::Gnd => return Err(no_model("it is a GND pin".into())),
            PinModel::Nc => return Err(no_model("it is an NC pin".into())),
            PinModel::Named(name) => file
                .pin_model(name, choice)
                .map_err(|e| no_model(e.to_string()))?,
        };
        Ok(Self::new(model, &Parasitics::of_pin(c, p, corner), corner)?)
    }

    /// The name of the model.
    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// The type of the model.
    pub fn model_type(&self) -> ModelType {
        self.model_type
    }

    /// The corner of the values.
    pub fn corner(&self) -> Corner {
        self.corner
    }

    /// Whether the model drives its pin (all the supported types but `Input` and `Terminator`).
    pub fn is_driver(&self) -> bool {
        self.edges.is_some()
    }

    /// The edges of a driver: from its `[Rising Waveform]` and `[Falling Waveform]`, or else
    /// linear from `[Ramp]`.
    pub fn edges(&self) -> Option<&Edges> {
        self.edges.as_ref()
    }

    /// What of the model was left out, such as waveforms with an unsupported fixture
    /// ([`cuprite_wire_ibis::spice::Subcircuit::notes`]).
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// The input thresholds of the model, to read levels from the voltage of the pin
    /// ([`InputCharacteristics::decode`]): `Vinl` and `Vinh` (of `[Model Spec]` at the corner if
    /// given, [`Model::thresholds`]), with `C_comp` as the capacitance and no pull or leakage.
    /// `None` if the model lacks `Vinl` or `Vinh`.
    pub fn input(&self) -> Option<&InputCharacteristics> {
        self.input.as_ref()
    }

    /// Whether the model can drive the level: `HiZ` only for 3-state, I/O and open types, `High`
    /// not for open drain and sink types, `Low` not for open source types.
    pub fn can_drive(&self, level: Level) -> bool {
        use ModelType::*;
        match level {
            Level::HiZ => self.is_driver() && self.model_type != Output,
            Level::High => {
                self.is_driver()
                    && !matches!(
                        self.model_type,
                        OpenDrain | IoOpenDrain | OpenSink | IoOpenSink
                    )
            }
            Level::Low => self.is_driver() && !matches!(self.model_type, OpenSource | IoOpenSource),
        }
    }

    /// The inline model of the subcircuit.
    fn model_source(&self) -> ModelSource {
        ModelSource::inline(self.name.clone(), self.text.clone())
    }
}

/// An IBIS output buffer playing a [`DigitalSequence`], with pins `out` and `gnd`.
///
/// It is written as a subcircuit of its own (`cuprite_ibis_<hash>`), holding the sources of the
/// weights `ku` and `kd` ([`Edges::weights`]) and the subcircuit of the buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct IbisDriver {
    buffer: IbisBuffer,
    sequence: DigitalSequence,
}

impl IbisDriver {
    /// The output pin.
    pub const OUT: &'static str = "out";
    /// The ground of the model, which its voltages are relative to.
    pub const GND: &'static str = "gnd";

    /// # Panics
    /// If the buffer does not drive (an `Input` model), or the sequence has a level the model
    /// cannot drive ([`IbisBuffer::can_drive`]).
    #[track_caller]
    pub fn new(buffer: IbisBuffer, sequence: DigitalSequence) -> Self {
        assert!(
            buffer.is_driver(),
            "the IBIS model `{}` ({}) does not drive",
            buffer.model_name,
            buffer.model_type
        );
        let levels =
            std::iter::once(sequence.initial()).chain(sequence.events().iter().map(|e| e.1));
        for level in levels {
            assert!(
                buffer.can_drive(level),
                "the IBIS model `{}` ({}) cannot drive {level:?}",
                buffer.model_name,
                buffer.model_type
            );
        }
        IbisDriver { buffer, sequence }
    }

    /// The buffer.
    pub fn buffer(&self) -> &IbisBuffer {
        &self.buffer
    }

    /// The levels the output plays.
    pub fn sequence(&self) -> &DigitalSequence {
        &self.sequence
    }

    /// The subcircuit name and definition ([`hashed_subcircuit`]).
    fn subcircuit(&self) -> (String, String) {
        let drive = |level| match level {
            Level::Low => Drive::Low,
            Level::High => Drive::High,
            Level::HiZ => Drive::HiZ,
        };
        let events: Vec<_> = self
            .sequence
            .events()
            .iter()
            .map(|&(time, level)| (time, drive(level)))
            .collect();
        let weights = self
            .buffer
            .edges
            .as_ref()
            .expect("a driver has edges")
            .weights(drive(self.sequence.initial()), &events);
        let reference = SUBCIRCUIT_REFERENCE;
        let body = format!(
            "VKU ku {reference} {}\nVKD kd {reference} {}\nXB {} {reference} ku kd {}\n",
            Waveform::Pwl(weights.ku).to_spice(),
            Waveform::Pwl(weights.kd).to_spice(),
            Self::OUT,
            self.buffer.name
        );
        hashed_subcircuit("ibis", &[Self::OUT], &body)
    }
}

impl Part for IbisDriver {
    fn ref_prefix(&self) -> String {
        "DRV".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        vec![Self::OUT.into(), Self::GND.into()]
    }
    fn value(&self) -> String {
        format!("IBIS {}", self.buffer.model_name)
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        let (name, text) = self.subcircuit();
        Some(SpicePartInfo::element(
            SpiceElement::new('X', [Self::OUT, Self::GND], name.clone())
                .with_model(self.buffer.model_source())
                .with_model(ModelSource::inline(name, text)),
        ))
    }
}

/// An IBIS input buffer, with pins `in` and `gnd`: the package, `C_comp` and the clamps (and the
/// resistors of a `Terminator` model).
///
/// A buffer that drives (an I/O model) is the receiver with its driver off. The levels are read
/// from the voltage of the pin with [`IbisBuffer::input`].
#[derive(Debug, Clone, PartialEq)]
pub struct IbisReceiver {
    buffer: IbisBuffer,
}

impl IbisReceiver {
    /// The input pin.
    pub const IN: &'static str = "in";
    /// The ground of the model, which its voltages are relative to.
    pub const GND: &'static str = "gnd";

    /// The receiver of the buffer.
    pub fn new(buffer: IbisBuffer) -> Self {
        IbisReceiver { buffer }
    }

    /// The buffer.
    pub fn buffer(&self) -> &IbisBuffer {
        &self.buffer
    }
}

impl Part for IbisReceiver {
    fn ref_prefix(&self) -> String {
        "RX".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        vec![Self::IN.into(), Self::GND.into()]
    }
    fn value(&self) -> String {
        format!("IBIS {}", self.buffer.model_name)
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        let pins = [Self::IN, Self::GND];
        let element = if self.buffer.is_driver() {
            // The weights of the driver at the reference: off.
            let reference = SUBCIRCUIT_REFERENCE;
            let body = format!(
                "XB {} {reference} {reference} {reference} {}\n",
                Self::IN,
                self.buffer.name
            );
            let (name, text) = hashed_subcircuit("ibis", &[Self::IN], &body);
            SpiceElement::new('X', pins, name.clone())
                .with_model(self.buffer.model_source())
                .with_model(ModelSource::inline(name, text))
        } else {
            SpiceElement::new('X', pins, self.buffer.name.clone())
                .with_model(self.buffer.model_source())
        };
        Some(SpicePartInfo::element(element))
    }
}

#[cfg(test)]
mod tests;
