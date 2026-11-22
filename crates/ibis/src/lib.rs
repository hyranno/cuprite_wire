//! Reads IBIS (I/O Buffer Information Specification) model files (`.ibs`).
//!
//! [`parse`] reads the text of a file into an [`IbisFile`]: the header, the components with their
//! package and pins, and the models with their I-V tables and ramp rates.
//!
//! ```
//! let ibs = cuprite_wire_ibis::parse(
//!     "\
//! [IBIS Ver] 2.1
//! [File Name] buffer.ibs
//! [Component] Example
//! [Manufacturer] Nobody
//! [Package]
//! R_pkg 50m NA NA
//! L_pkg 2n NA NA
//! C_pkg 1p NA NA
//! [Pin] signal_name model_name
//! 1 OUT buffer
//! 2 GND GND
//! [Model] buffer
//! Model_type Output
//! C_comp 2p NA NA
//! [Voltage Range] 3.3 3.0 3.6
//! [Pulldown]
//! 0 0 NA NA
//! 3.3 50m NA NA
//! [Pullup]
//! 0 0 NA NA
//! 3.3 -50m NA NA
//! [Ramp]
//! dV/dt_r 2.0/1n NA NA
//! dV/dt_f 2.0/1n NA NA
//! [END]
//! ",
//! )?;
//! let model = ibs.model("buffer").unwrap();
//! assert_eq!(model.model_type, cuprite_wire_ibis::ModelType::Output);
//! assert_eq!(model.voltage_range.unwrap().typ, 3.3);
//! # Ok::<(), cuprite_wire_ibis::Error>(())
//! ```
//!
//! # Coverage
//!
//! The parser follows the syntax of the IBIS specification (up to version 7): keywords are
//! case-insensitive and treat `_` and spaces alike, `[Comment Char]` changes the comment
//! character, numbers take scale suffixes (`p`, `n`, `m`, `k`, `M`, ...) and ignore the units
//! after them, and `NA` marks missing min/max values. It reads:
//!
//! - the header: `[IBIS Ver]`, `[Comment Char]`, `[File Name]`, `[File Rev]`, `[Date]`,
//!   `[Source]`, `[Notes]`, `[Disclaimer]`, `[Copyright]`;
//! - `[Component]` with `[Manufacturer]`, `[Package]` and `[Pin]`;
//! - `[Model]` with its subparameters (`Model_type`, `Polarity`, `Enable`, `Vinl`, `Vinh`,
//!   `Vmeas`, `Vref`, `Cref`, `Rref`, `C_comp`), `[Temperature Range]`, `[Voltage Range]`, the
//!   references (`[Pullup Reference]`, ...), the I-V tables (`[Pullup]`, `[Pulldown]`,
//!   `[GND Clamp]`, `[POWER Clamp]`) and `[Ramp]`.
//!
//! Other keywords and subparameters (`[Rising Waveform]`, `[Model Selector]`, `[Diff Pin]`,
//! `[Package Model]`, `[Submodel]`, IBIS-AMI, ...) are skipped without an error and listed in
//! [`IbisFile::skipped`]. The parser is lenient about what does not change the meaning: it does
//! not check line lengths, and keywords may be indented.
//!
//! # Simulation
//!
//! [`spice::subcircuit`] writes a model as a SPICE subcircuit for ngspice, with ports that switch
//! its driver from outside, so that a test bench can play any sequence of states on it.

#![warn(missing_docs, clippy::missing_errors_doc, clippy::missing_panics_doc)]

mod lex;
mod number;
mod parse;
pub mod spice;

use std::fmt;

pub use parse::parse;

/// The contents of an IBIS file.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct IbisFile {
    /// The keywords describing the file.
    pub header: Header,
    /// The `[Component]`s, in the order of the file.
    pub components: Vec<Component>,
    /// The `[Model]`s, in the order of the file.
    pub models: Vec<Model>,
    /// The names of the `[Model Selector]`s, which pins may name instead of a model. Their
    /// contents are not read.
    pub model_selectors: Vec<String>,
    /// The keywords and subparameters that were skipped, in order.
    pub skipped: Vec<Skipped>,
}

impl IbisFile {
    /// The component with the name.
    pub fn component(&self, name: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.name == name)
    }

    /// The model with the name.
    pub fn model(&self, name: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.name == name)
    }
}

/// The keywords describing the file itself.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Header {
    /// The version of the specification the file follows (`[IBIS Ver]`, e.g. `"5.0"`).
    pub ibis_ver: String,
    /// The comment character in effect at the end of the file (`|` unless `[Comment Char]`
    /// changed it).
    pub comment_char: char,
    /// `[File Name]`.
    pub file_name: Option<String>,
    /// `[File Rev]`.
    pub file_rev: Option<String>,
    /// `[Date]`, as written.
    pub date: Option<String>,
    /// `[Source]`, with its lines joined by `\n` (as are the texts below).
    pub source: Option<String>,
    /// `[Notes]`.
    pub notes: Option<String>,
    /// `[Disclaimer]`.
    pub disclaimer: Option<String>,
    /// `[Copyright]`.
    pub copyright: Option<String>,
}

/// A `[Component]`: a device and its pins.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Component {
    /// The name after `[Component]`.
    pub name: String,
    /// `[Manufacturer]`.
    pub manufacturer: Option<String>,
    /// `[Package]` (all `None` when absent).
    pub package: Package,
    /// The rows of `[Pin]`, in order.
    pub pins: Vec<Pin>,
}

impl Component {
    /// The pin with the name (pin number).
    pub fn pin(&self, name: &str) -> Option<&Pin> {
        self.pins.iter().find(|p| p.name == name)
    }
}

/// The default package parasitics of a component (`[Package]`), for pins without their own.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Package {
    /// `R_pkg`, in ohms.
    pub r_pkg: Option<TypMinMax<f64>>,
    /// `L_pkg`, in henries.
    pub l_pkg: Option<TypMinMax<f64>>,
    /// `C_pkg`, in farads.
    pub c_pkg: Option<TypMinMax<f64>>,
}

/// A row of `[Pin]`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Pin {
    /// The pin name, usually the pin number.
    pub name: String,
    /// The name of the signal on the pin.
    pub signal_name: String,
    /// What the pin connects to.
    pub model: PinModel,
    /// The resistance of this pin, in ohms, overriding `R_pkg` (`None` when absent or `NA`).
    pub r_pin: Option<f64>,
    /// The inductance of this pin, in henries, overriding `L_pkg`.
    pub l_pin: Option<f64>,
    /// The capacitance of this pin, in farads, overriding `C_pkg`.
    pub c_pin: Option<f64>,
}

/// What a pin connects to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinModel {
    /// A power supply pin (`POWER`).
    Power,
    /// A ground pin (`GND`).
    Gnd,
    /// A pin without connection (`NC`).
    Nc,
    /// A [`Model`] or a `[Model Selector]`, by name.
    Named(String),
}

/// A `[Model]`: the electrical behavior of a buffer.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Model {
    /// The name after `[Model]`.
    pub name: String,
    /// `Model_type`.
    pub model_type: ModelType,
    /// `Polarity`.
    pub polarity: Option<Polarity>,
    /// `Enable`.
    pub enable: Option<Enable>,
    /// `Vinl`: the highest input voltage read as low, in volts.
    pub vinl: Option<f64>,
    /// `Vinh`: the lowest input voltage read as high, in volts.
    pub vinh: Option<f64>,
    /// `Vmeas`: the voltage at which the timing is measured, in volts.
    pub vmeas: Option<f64>,
    /// `Vref`: the voltage the timing test load connects to, in volts.
    pub vref: Option<f64>,
    /// `Cref`: the capacitance of the timing test load, in farads.
    pub cref: Option<f64>,
    /// `Rref`: the resistance of the timing test load, in ohms.
    pub rref: Option<f64>,
    /// `C_comp`: the die capacitance, in farads.
    pub c_comp: Option<TypMinMax<f64>>,
    /// `[Temperature Range]`, in degrees Celsius.
    pub temperature_range: Option<TypMinMax<f64>>,
    /// `[Voltage Range]`: the supply voltage, when the references below are absent, in volts.
    pub voltage_range: Option<TypMinMax<f64>>,
    /// `[Pullup Reference]`, in volts.
    pub pullup_reference: Option<TypMinMax<f64>>,
    /// `[Pulldown Reference]`, in volts.
    pub pulldown_reference: Option<TypMinMax<f64>>,
    /// `[POWER Clamp Reference]`, in volts.
    pub power_clamp_reference: Option<TypMinMax<f64>>,
    /// `[GND Clamp Reference]`, in volts.
    pub gnd_clamp_reference: Option<TypMinMax<f64>>,
    /// `[Pullup]`. Its voltages, like those of `[POWER Clamp]`, are relative to the reference
    /// (the supply voltage minus the pin voltage); those of `[Pulldown]` and `[GND Clamp]` are
    /// relative to ground.
    pub pullup: Option<IvTable>,
    /// `[Pulldown]`.
    pub pulldown: Option<IvTable>,
    /// `[GND Clamp]`.
    pub gnd_clamp: Option<IvTable>,
    /// `[POWER Clamp]`.
    pub power_clamp: Option<IvTable>,
    /// `[Ramp]`.
    pub ramp: Option<Ramp>,
}

/// `Model_type` of a [`Model`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ModelType {
    /// `Input`.
    Input,
    /// `Output`.
    Output,
    /// `I/O`.
    Io,
    /// `3-state`.
    ThreeState,
    /// `Open_drain`.
    OpenDrain,
    /// `I/O_open_drain`.
    IoOpenDrain,
    /// `Open_sink`.
    OpenSink,
    /// `I/O_open_sink`.
    IoOpenSink,
    /// `Open_source`.
    OpenSource,
    /// `I/O_open_source`.
    IoOpenSource,
    /// `Input_ECL`.
    InputEcl,
    /// `Output_ECL`.
    OutputEcl,
    /// `I/O_ECL`.
    IoEcl,
    /// `3-state_ECL`.
    ThreeStateEcl,
    /// `Terminator`.
    Terminator,
    /// `Series`.
    Series,
    /// `Series_switch`.
    SeriesSwitch,
    /// `Input_diff`.
    InputDiff,
    /// `Output_diff`.
    OutputDiff,
    /// `I/O_diff`.
    IoDiff,
    /// `3-state_diff`.
    ThreeStateDiff,
}

impl ModelType {
    const NAMES: [(&str, ModelType); 21] = [
        ("Input", ModelType::Input),
        ("Output", ModelType::Output),
        ("I/O", ModelType::Io),
        ("3-state", ModelType::ThreeState),
        ("Open_drain", ModelType::OpenDrain),
        ("I/O_open_drain", ModelType::IoOpenDrain),
        ("Open_sink", ModelType::OpenSink),
        ("I/O_open_sink", ModelType::IoOpenSink),
        ("Open_source", ModelType::OpenSource),
        ("I/O_open_source", ModelType::IoOpenSource),
        ("Input_ECL", ModelType::InputEcl),
        ("Output_ECL", ModelType::OutputEcl),
        ("I/O_ECL", ModelType::IoEcl),
        ("3-state_ECL", ModelType::ThreeStateEcl),
        ("Terminator", ModelType::Terminator),
        ("Series", ModelType::Series),
        ("Series_switch", ModelType::SeriesSwitch),
        ("Input_diff", ModelType::InputDiff),
        ("Output_diff", ModelType::OutputDiff),
        ("I/O_diff", ModelType::IoDiff),
        ("3-state_diff", ModelType::ThreeStateDiff),
    ];

    /// The type of the name, case-insensitive.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::NAMES
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|&(_, t)| t)
    }

    /// The name as written in the specification (`"I/O_open_drain"`).
    pub fn name(self) -> &'static str {
        Self::NAMES
            .iter()
            .find(|&&(_, t)| t == self)
            .map(|&(n, _)| n)
            .unwrap_or_default()
    }
}

impl fmt::Display for ModelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// `Polarity` of a [`Model`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Polarity {
    /// `Non-Inverting`: the output follows the input.
    NonInverting,
    /// `Inverting`.
    Inverting,
}

/// `Enable` of a [`Model`]: the level of the enable input that turns the driver on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Enable {
    /// `Active-High`.
    ActiveHigh,
    /// `Active-Low`.
    ActiveLow,
}

/// One of the three columns of a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Corner {
    /// The typical value.
    #[default]
    Typ,
    /// The minimum: the slow, weak corner (for currents, the smallest magnitude).
    Min,
    /// The maximum: the fast, strong corner.
    Max,
}

/// A value with typical, minimum and maximum columns. The minimum and maximum may be `NA`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypMinMax<T> {
    /// The typical value.
    pub typ: T,
    /// The minimum, `None` for `NA`.
    pub min: Option<T>,
    /// The maximum, `None` for `NA`.
    pub max: Option<T>,
}

impl<T: Copy> TypMinMax<T> {
    /// The value of the corner; the typical value when the column is `NA`.
    pub fn get(&self, corner: Corner) -> T {
        match corner {
            Corner::Typ => self.typ,
            Corner::Min => self.min.unwrap_or(self.typ),
            Corner::Max => self.max.unwrap_or(self.typ),
        }
    }
}

/// An I-V table (`[Pullup]`, `[Pulldown]`, `[GND Clamp]`, `[POWER Clamp]`).
#[derive(Debug, Clone, PartialEq)]
pub struct IvTable {
    /// The rows, in the order of the file.
    pub points: Vec<IvPoint>,
}

/// A row of an [`IvTable`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IvPoint {
    /// In volts.
    pub voltage: f64,
    /// The current into the pin at the voltage, in amperes.
    pub current: TypMinMax<f64>,
}

/// `[Ramp]`: the rise and fall rates of a driver, measured from 20% to 80% of the swing into
/// `r_load`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Ramp {
    /// `dV/dt_r`: the rising edge.
    pub dv_dt_r: TypMinMax<Slope>,
    /// `dV/dt_f`: the falling edge.
    pub dv_dt_f: TypMinMax<Slope>,
    /// `R_load`: the test load, in ohms (`None` for the default of 50 ohms).
    pub r_load: Option<f64>,
}

/// A rate given as a voltage change over a time (`dV/dt`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slope {
    /// In volts.
    pub dv: f64,
    /// In seconds.
    pub dt: f64,
}

/// A keyword or subparameter the parser does not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// The keyword with its brackets (`[Rising Waveform]`) or the subparameter
    /// (`C_comp_pullup`), as written.
    pub name: String,
    /// The line number (1-based).
    pub line: usize,
}

/// An error reading a file, at a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// The line number (1-based).
    pub line: usize,
    /// What is wrong.
    pub message: String,
}

impl Error {
    pub(crate) fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for Error {}
