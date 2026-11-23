//! Simulation-only parts of IBIS models (feature `ibis`): a driver that plays a
//! [`DigitalSequence`] through the I-V tables and ramps of an output buffer, and the receiver of
//! an input buffer, single-ended or differential.
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
//!         ibis::{IbisBuffer, IbisDriver, IbisReceiver, read_file},
//!     },
//! };
//!
//! let ibs = read_file("models/mcu.ibs")?;
//! let tx = IbisBuffer::of_pin(&ibs, "MCU", "12", Corner::Typ)?;
//! let rx = IbisBuffer::of_pin(&ibs, "MCU", "13", Corner::Typ)?;
//! let driver = IbisDriver::new(tx, DigitalSequence::new(Level::High).uart(1e-6, 1e6, b"Hi"));
//! let receiver = IbisReceiver::new(rx.clone());
//! // ... connect them, simulate, and read the line with `rx.input().unwrap().decode(...)`.
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! [`ibis_part!`](crate::ibis_part) checks the pins at compile time and generates functions
//! returning their buffers in place of the `of_pin` calls.
//!
//! # Differential pairs
//!
//! An [`IbisDiffBuffer`] is a pair of `[Diff Pin]`: the buffers of its non-inverting and
//! inverting pins, the differential threshold `vdiff` and the delay `tdelay` of the inverting
//! pin. [`IbisDiffDriver`] drives the inverting pin with the complement of the sequence, and
//! [`IbisDiffReceiver`] puts the receivers on both pins; the levels are read from the voltage
//! between them against [`IbisDiffBuffer::vdiff`]. A `Series` model that the component maps
//! between the two pins (`[Series Pin Mapping]`, such as an on-die termination) is part of both;
//! [`IbisSeries`] is such a model as a part of its own.
//!
//! `Polarity` and `Enable` of the models are ignored: the sequence is the level of the pad (of
//! the non-inverting pin of a pair).

use std::{
    fmt,
    path::{Path, PathBuf},
};

use cuprite_wire_ibis::{
    Corner, IbisFile, LookupError, Model, ModelType,
    spice::{Drive, Edges, Parasitics, SubcircuitError, series_subcircuit, subcircuit},
};

use crate::{
    parts::Part,
    spice::{
        ModelSource, SpiceElement, SpicePartInfo, Waveform,
        digital::{DigitalSequence, InputCharacteristics, Level},
        subcircuit::{SUBCIRCUIT_REFERENCE, fnv1a, hashed_subcircuit},
    },
};

/// The reason [`IbisBuffer::of_pin`] (or [`read_file`]) has no buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IbisBufferError {
    /// The file cannot be read or is not a valid IBIS file.
    Read {
        /// The file.
        path: PathBuf,
        /// Why it cannot be read.
        message: String,
    },
    /// The file has no such component, pin, model or pair.
    Lookup(LookupError),
    /// The model cannot be simulated.
    Subcircuit(SubcircuitError),
}

impl fmt::Display for IbisBufferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IbisBufferError::Read { path, message } => {
                write!(f, "cannot read {}: {message}", path.display())
            }
            IbisBufferError::Lookup(error) => error.fmt(f),
            IbisBufferError::Subcircuit(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for IbisBufferError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IbisBufferError::Read { .. } => None,
            IbisBufferError::Lookup(error) => Some(error),
            IbisBufferError::Subcircuit(error) => Some(error),
        }
    }
}

impl From<LookupError> for IbisBufferError {
    fn from(error: LookupError) -> Self {
        IbisBufferError::Lookup(error)
    }
}

impl From<SubcircuitError> for IbisBufferError {
    fn from(error: SubcircuitError) -> Self {
        IbisBufferError::Subcircuit(error)
    }
}

/// Reads and parses the IBIS file at `path`. Bytes that are not UTF-8 (in the comments of some
/// vendor files) are replaced.
///
/// # Errors
/// [`IbisBufferError::Read`] if the file cannot be read or parsed.
pub fn read_file(path: impl AsRef<Path>) -> Result<IbisFile, IbisBufferError> {
    let path = path.as_ref();
    let error = |message: String| IbisBufferError::Read {
        path: path.to_path_buf(),
        message,
    };
    let bytes = std::fs::read(path).map_err(|e| error(e.to_string()))?;
    cuprite_wire_ibis::parse(&String::from_utf8_lossy(&bytes)).map_err(|e| error(e.to_string()))
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
        let c = file.find_component(component)?;
        let (p, model) = file.buffer_model(c, pin, choice)?;
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

    /// Whether the model drives its pin (all the supported types but `Input`, `Input_ECL` and
    /// `Terminator`).
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

    /// Whether the model can drive the level: `HiZ` only for 3-state, I/O and open types (not
    /// `Output` and `Output_ECL`), `High` not for open drain and sink types, `Low` not for open
    /// source types.
    pub fn can_drive(&self, level: Level) -> bool {
        use ModelType::*;
        match level {
            Level::HiZ => self.is_driver() && !matches!(self.model_type, Output | OutputEcl),
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

    /// Panics unless the buffer drives and can drive the levels.
    #[track_caller]
    fn assert_drives(&self, levels: impl IntoIterator<Item = Level>) {
        assert!(
            self.is_driver(),
            "the IBIS model `{}` ({}) does not drive",
            self.model_name,
            self.model_type
        );
        for level in levels {
            assert!(
                self.can_drive(level),
                "the IBIS model `{}` ({}) cannot drive {level:?}",
                self.model_name,
                self.model_type
            );
        }
    }

    /// The lines of a wrapper subcircuit playing `sequence`, each level mapped by `level`,
    /// delayed by `delay`, on the node `pin`: the sources of the weights and the buffer, with `id`
    /// after the names of the elements and the nodes of the weights.
    fn driver_lines(
        &self,
        sequence: &DigitalSequence,
        level: impl Fn(Level) -> Level,
        delay: f64,
        pin: &str,
        id: &str,
    ) -> String {
        let drive = |l| match level(l) {
            Level::Low => Drive::Low,
            Level::High => Drive::High,
            Level::HiZ => Drive::HiZ,
        };
        let events: Vec<_> = sequence
            .events()
            .iter()
            .map(|&(time, level)| (time + delay, drive(level)))
            .collect();
        let weights = self
            .edges
            .as_ref()
            .expect("a driver has edges")
            .weights(drive(sequence.initial()), &events);
        let reference = SUBCIRCUIT_REFERENCE;
        format!(
            "VKU{id} ku{id} {reference} {}\nVKD{id} kd{id} {reference} {}\nXB{id} {pin} {reference} ku{id} kd{id} {}\n",
            Waveform::Pwl(weights.ku).to_spice(),
            Waveform::Pwl(weights.kd).to_spice(),
            self.name
        )
    }

    /// The line of a wrapper subcircuit with the receiver on the node `pin`, with `id` after the
    /// name of the element: a driver with its weights at the reference (off).
    fn receiver_line(&self, pin: &str, id: &str) -> String {
        let reference = SUBCIRCUIT_REFERENCE;
        if self.is_driver() {
            format!(
                "XB{id} {pin} {reference} {reference} {reference} {}\n",
                self.name
            )
        } else {
            format!("XB{id} {pin} {reference} {}\n", self.name)
        }
    }
}

/// A `Series` model of an IBIS file, at a corner, as a part with pins `a` and `b`: its passive
/// elements between them ([`series_subcircuit`]).
#[derive(Debug, Clone, PartialEq)]
pub struct IbisSeries {
    model_name: String,
    corner: Corner,
    /// The name of the subcircuit, `cuprite_ibis_series_<hash>`.
    name: String,
    text: String,
}

impl IbisSeries {
    /// The first pin.
    pub const A: &'static str = "a";
    /// The second pin.
    pub const B: &'static str = "b";

    /// The series element of `model`, with the values of `corner`.
    ///
    /// # Errors
    /// If the model is not a `Series` model of passive elements
    /// ([`cuprite_wire_ibis::spice::series_subcircuit`]).
    pub fn new(model: &Model, corner: Corner) -> Result<Self, SubcircuitError> {
        let hash = fnv1a(series_subcircuit("", model, corner)?.text.as_bytes());
        let name = format!("cuprite_ibis_series_{hash:016x}");
        let subckt = series_subcircuit(&name, model, corner)?;
        Ok(IbisSeries {
            model_name: model.name.clone(),
            corner,
            name,
            text: subckt.text,
        })
    }

    /// The model that `[Series Pin Mapping]` of the component `component` of `file` puts between
    /// the pins `pin1` and `pin2` (in either order). The packages of the pins are not part of it.
    ///
    /// # Errors
    /// If the file has no such component, no row of `[Series Pin Mapping]` joins the pins, or
    /// [`IbisSeries::new`] fails.
    pub fn of_pins(
        file: &IbisFile,
        component: &str,
        pin1: &str,
        pin2: &str,
        corner: Corner,
    ) -> Result<Self, IbisBufferError> {
        let c = file.find_component(component)?;
        Self::of_component(file, c, pin1, pin2, corner)?.ok_or_else(|| {
            LookupError::NoSeriesPinMapping {
                component: component.to_string(),
                pin1: pin1.to_string(),
                pin2: pin2.to_string(),
            }
            .into()
        })
    }

    /// The model between the pins of the component, `None` if no row joins them.
    fn of_component(
        file: &IbisFile,
        component: &cuprite_wire_ibis::Component,
        pin1: &str,
        pin2: &str,
        corner: Corner,
    ) -> Result<Option<Self>, IbisBufferError> {
        let Some(model) = file.series_model(component, pin1, pin2)? else {
            return Ok(None);
        };
        Ok(Some(Self::new(model, corner)?))
    }

    /// The name of the model.
    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// The corner of the values.
    pub fn corner(&self) -> Corner {
        self.corner
    }

    /// The inline model of the subcircuit.
    fn model_source(&self) -> ModelSource {
        ModelSource::inline(self.name.clone(), self.text.clone())
    }
}

impl Part for IbisSeries {
    fn ref_prefix(&self) -> String {
        "SER".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        vec![Self::A.into(), Self::B.into()]
    }
    fn value(&self) -> String {
        format!("IBIS {}", self.model_name)
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        Some(SpicePartInfo::element(
            SpiceElement::new('X', [Self::A, Self::B], self.name.clone())
                .with_model(self.model_source()),
        ))
    }
}

/// A differential pair of IBIS buffers: the buffers of the non-inverting and inverting pins of a
/// row of `[Diff Pin]`, with its threshold and delay, and the series model between the pins, if
/// any.
#[derive(Debug, Clone, PartialEq)]
pub struct IbisDiffBuffer {
    p: IbisBuffer,
    n: IbisBuffer,
    vdiff: Option<f64>,
    tdelay: f64,
    termination: Option<IbisSeries>,
}

impl IbisDiffBuffer {
    /// The pair of the buffers `p` (non-inverting) and `n` (inverting), with the differential
    /// threshold `vdiff` (in volts) and the delay `tdelay` of `n` after `p` (in seconds; negative
    /// if `n` switches first), without a termination.
    pub fn new(p: IbisBuffer, n: IbisBuffer, vdiff: Option<f64>, tdelay: f64) -> Self {
        IbisDiffBuffer {
            p,
            n,
            vdiff,
            tdelay,
            termination: None,
        }
    }

    /// Puts the series element between the pins, such as an on-die termination.
    pub fn with_termination(mut self, termination: IbisSeries) -> Self {
        self.termination = Some(termination);
        self
    }

    /// The pair of `[Diff Pin]` of the component `component` of `file` that has the pin `pin`
    /// (either one): the buffers of its pins ([`IbisBuffer::of_pin`]), `vdiff`, `tdelay` at the
    /// corner (0 when `NA`), and the `Series` model that `[Series Pin Mapping]` puts between the
    /// pins, if any.
    ///
    /// # Errors
    /// If the file has no such component, the pin is not in `[Diff Pin]`, a buffer fails
    /// ([`IbisBuffer::of_pin`]), or the series model fails ([`IbisSeries::new`]).
    pub fn of_pin(
        file: &IbisFile,
        component: &str,
        pin: &str,
        corner: Corner,
    ) -> Result<Self, IbisBufferError> {
        Self::pair(file, component, pin, None, corner)
    }

    /// The pair as [`IbisDiffBuffer::of_pin`], with the model `model` of the `[Model Selector]`
    /// of both pins.
    ///
    /// # Errors
    /// As [`IbisDiffBuffer::of_pin`], and as [`IbisBuffer::of_pin_model`].
    pub fn of_pin_model(
        file: &IbisFile,
        component: &str,
        pin: &str,
        model: &str,
        corner: Corner,
    ) -> Result<Self, IbisBufferError> {
        Self::pair(file, component, pin, Some(model), corner)
    }

    fn pair(
        file: &IbisFile,
        component: &str,
        pin: &str,
        choice: Option<&str>,
        corner: Corner,
    ) -> Result<Self, IbisBufferError> {
        let c = file.find_component(component)?;
        let diff = c.find_diff_pin(pin)?;
        let buffer = |pin| IbisBuffer::pin_buffer(file, component, pin, choice, corner);
        Ok(IbisDiffBuffer {
            p: buffer(&diff.pin)?,
            n: buffer(&diff.inv_pin)?,
            vdiff: diff.vdiff,
            tdelay: diff.tdelay.map_or(0.0, |t| t.get(corner)),
            termination: IbisSeries::of_component(file, c, &diff.pin, &diff.inv_pin, corner)?,
        })
    }

    /// The buffer of the non-inverting pin.
    pub fn p(&self) -> &IbisBuffer {
        &self.p
    }

    /// The buffer of the inverting pin.
    pub fn n(&self) -> &IbisBuffer {
        &self.n
    }

    /// The differential threshold of a receiver, in volts: the input is high when the voltage of
    /// the non-inverting pin minus that of the inverting pin is above `vdiff`, and low below
    /// `-vdiff`. `None` when the file gives `NA`.
    pub fn vdiff(&self) -> Option<f64> {
        self.vdiff
    }

    /// The delay of the inverting pin after the non-inverting one when driving, in seconds.
    pub fn tdelay(&self) -> f64 {
        self.tdelay
    }

    /// The series element between the pins.
    pub fn termination(&self) -> Option<&IbisSeries> {
        self.termination.as_ref()
    }

    /// The inline models of the buffers and the termination.
    fn model_sources(&self) -> Vec<ModelSource> {
        let mut models = vec![self.p.model_source(), self.n.model_source()];
        models.extend(self.termination.as_ref().map(IbisSeries::model_source));
        models
    }

    /// The line of the termination of a wrapper subcircuit, if any.
    fn termination_line(&self) -> String {
        self.termination
            .as_ref()
            .map(|t| format!("XT {} {} {}\n", DIFF_P, DIFF_N, t.name))
            .unwrap_or_default()
    }

    /// The SPICE element of a part with the wrapper subcircuit `body`.
    fn element(&self, body: &str) -> SpicePartInfo {
        let (name, text) = hashed_subcircuit("ibis", &[DIFF_P, DIFF_N], body);
        let mut element = SpiceElement::new('X', [DIFF_P, DIFF_N, DIFF_GND], name.clone());
        for model in self.model_sources() {
            element = element.with_model(model);
        }
        SpicePartInfo::element(element.with_model(ModelSource::inline(name, text)))
    }
}

const DIFF_P: &str = "p";
const DIFF_N: &str = "n";
const DIFF_GND: &str = "gnd";

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
        buffer.assert_drives(levels(&sequence));
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
        let body = self
            .buffer
            .driver_lines(&self.sequence, |l| l, 0.0, Self::OUT, "");
        hashed_subcircuit("ibis", &[Self::OUT], &body)
    }
}

/// The levels of the sequence, the initial one included.
fn levels(sequence: &DigitalSequence) -> impl Iterator<Item = Level> + '_ {
    std::iter::once(sequence.initial()).chain(sequence.events().iter().map(|e| e.1))
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
            let body = self.buffer.receiver_line(Self::IN, "");
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

/// A differential IBIS driver playing a [`DigitalSequence`], with pins `p`, `n` and `gnd`: the
/// non-inverting pin plays the sequence, the inverting pin its complement (`HiZ` stays `HiZ`)
/// delayed by [`IbisDiffBuffer::tdelay`] (or the non-inverting pin delayed when it is negative).
/// A pin whose buffer cannot drive `High` (open drain and sink types) or `Low` (open source
/// types) is released instead, as in current-mode pairs such as TMDS.
///
/// It is written as a subcircuit of its own (`cuprite_ibis_<hash>`), holding the sources of the
/// weights of both buffers, the buffers and the termination.
#[derive(Debug, Clone, PartialEq)]
pub struct IbisDiffDriver {
    buffer: IbisDiffBuffer,
    sequence: DigitalSequence,
}

impl IbisDiffDriver {
    /// The non-inverting pin.
    pub const P: &'static str = DIFF_P;
    /// The inverting pin.
    pub const N: &'static str = DIFF_N;
    /// The ground of the models, which their voltages are relative to.
    pub const GND: &'static str = DIFF_GND;

    /// # Panics
    /// If a buffer does not drive, or the sequence has `HiZ` and a buffer cannot release
    /// ([`IbisBuffer::can_drive`]).
    #[track_caller]
    pub fn new(buffer: IbisDiffBuffer, sequence: DigitalSequence) -> Self {
        for (b, invert) in [(&buffer.p, false), (&buffer.n, true)] {
            b.assert_drives(levels(&sequence).map(|l| pad_level(b, l, invert)));
        }
        IbisDiffDriver { buffer, sequence }
    }

    /// The buffers.
    pub fn buffer(&self) -> &IbisDiffBuffer {
        &self.buffer
    }

    /// The levels the non-inverting pin plays.
    pub fn sequence(&self) -> &DigitalSequence {
        &self.sequence
    }

    /// The body of the wrapper subcircuit.
    fn body(&self) -> String {
        let b = &self.buffer;
        let (delay_p, delay_n) = if b.tdelay >= 0.0 {
            (0.0, b.tdelay)
        } else {
            (-b.tdelay, 0.0)
        };
        let p = |l| pad_level(&b.p, l, false);
        let n = |l| pad_level(&b.n, l, true);
        b.p.driver_lines(&self.sequence, p, delay_p, DIFF_P, "P")
            + &b.n.driver_lines(&self.sequence, n, delay_n, DIFF_N, "N")
            + &b.termination_line()
    }
}

/// The level a pin of a differential driver plays for the level of the sequence: complemented
/// on the inverting pin, and released where the buffer cannot drive it.
fn pad_level(buffer: &IbisBuffer, level: Level, invert: bool) -> Level {
    let level = match (level, invert) {
        (Level::Low, true) => Level::High,
        (Level::High, true) => Level::Low,
        (level, _) => level,
    };
    if level != Level::HiZ && !buffer.can_drive(level) {
        Level::HiZ
    } else {
        level
    }
}

impl Part for IbisDiffDriver {
    fn ref_prefix(&self) -> String {
        "DRV".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        vec![Self::P.into(), Self::N.into(), Self::GND.into()]
    }
    fn value(&self) -> String {
        format!("IBIS {}", self.buffer.p.model_name)
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        Some(self.buffer.element(&self.body()))
    }
}

/// A differential IBIS receiver, with pins `p`, `n` and `gnd`: the receivers of both buffers
/// (as [`IbisReceiver`]) and the termination.
///
/// The levels are read from the voltage of `p` minus that of `n` against
/// [`IbisDiffBuffer::vdiff`].
#[derive(Debug, Clone, PartialEq)]
pub struct IbisDiffReceiver {
    buffer: IbisDiffBuffer,
}

impl IbisDiffReceiver {
    /// The non-inverting pin.
    pub const P: &'static str = DIFF_P;
    /// The inverting pin.
    pub const N: &'static str = DIFF_N;
    /// The ground of the models, which their voltages are relative to.
    pub const GND: &'static str = DIFF_GND;

    /// The receiver of the buffers.
    pub fn new(buffer: IbisDiffBuffer) -> Self {
        IbisDiffReceiver { buffer }
    }

    /// The buffers.
    pub fn buffer(&self) -> &IbisDiffBuffer {
        &self.buffer
    }
}

impl Part for IbisDiffReceiver {
    fn ref_prefix(&self) -> String {
        "RX".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        vec![Self::P.into(), Self::N.into(), Self::GND.into()]
    }
    fn value(&self) -> String {
        format!("IBIS {}", self.buffer.p.model_name)
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        let b = &self.buffer;
        let body = b.p.receiver_line(DIFF_P, "P")
            + &b.n.receiver_line(DIFF_N, "N")
            + &b.termination_line();
        Some(b.element(&body))
    }
}

#[cfg(test)]
mod tests;
