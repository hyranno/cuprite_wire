//! XSPICE co-simulation: a digital model in a shared library, run by ngspice's `d_cosim` code
//! model, with its ports bridged to the analog pins of a part.
//!
//! The library implements `Cosim_setup` of ngspice's `cosim.h` (shipped with ngspice in
//! `/usr/share/ngspice/scripts/src/ngspice/cosim.h`): a model written in C, or a Verilog design
//! built with [`Verilator`]. Unlike `spice::cosim` (Rust models, with the `shared-ngspice`
//! feature), it runs in the ngspice CLI as well.

use std::path::PathBuf;

use crate::{
    parts::Part,
    spice::{
        SpicePartInfo,
        digital::{
            InputCharacteristics, OutputCharacteristics, SUBCIRCUIT_REFERENCE,
            assert_subcircuit_pin, hashed_subcircuit, internal_node, subcircuit_part,
        },
        sources::spice_number,
    },
};

/// A digital model in a shared library (`d_cosim`), with pins for its input and output ports
/// and `gnd`, the reference of their voltages.
///
/// Each input pin has the load of its [`InputCharacteristics`] and an `adc_bridge` at its
/// thresholds (a voltage between them is the unknown state); each output pin a `dac_bridge` from
/// `low` to `high` with the edges of its [`OutputCharacteristics`], behind the output resistance.
/// The ports of the library are the inputs and the outputs in the order they are added
/// (bit 0 is the first input or output). Bidirectional ports and high impedance are not supported.
///
/// It is written as a subcircuit `cuprite_dcosim_<hash>` of its own, whose node `ref` is the
/// `gnd` pin. The bridges work against the ground, so they are connected through unity-gain
/// sources relative to `ref`. The internal nodes of a pin `p` are named `p#<role>` (`p#ai`,
/// `p#di`, `p#do`, `p#ao`, `p#d`), so they never meet a pin's node.
#[derive(Debug, Clone, PartialEq)]
pub struct DCosim {
    simulation: PathBuf,
    sim_args: Vec<String>,
    lib_args: Vec<String>,
    delay: Option<f64>,
    inputs: Vec<(String, InputCharacteristics)>,
    outputs: Vec<(String, OutputCharacteristics)>,
}

/// An argument written as a quoted string of the model.
#[track_caller]
fn check_string(what: &str, value: &str) {
    assert!(
        !value.contains(|c: char| c == '"' || c.is_control()),
        "the {what} {value:?} cannot be written in a model (it contains '\"' or a control character)"
    );
}

/// Collects arguments written as quoted strings of the model, checking each with [`check_string`].
#[track_caller]
fn string_args<S: Into<String>>(what: &str, args: impl IntoIterator<Item = S>) -> Vec<String> {
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    for arg in &args {
        check_string(what, arg);
    }
    args
}

impl DCosim {
    /// Name of the pin that is the reference of the port voltages.
    pub const GND: &'static str = "gnd";

    /// A model in the library at `simulation` (the `simulation` parameter of `d_cosim`).
    ///
    /// # Panics
    /// If the path is not absolute (ngspice would resolve it against its working directory) or
    /// cannot be written in a model.
    #[track_caller]
    pub fn new(simulation: impl Into<PathBuf>) -> Box<Self> {
        let simulation = simulation.into();
        assert!(
            simulation.is_absolute(),
            "the library path {} is not absolute",
            simulation.display()
        );
        check_string(
            "library path",
            simulation.to_str().expect("the library path is UTF-8"),
        );
        Box::new(DCosim {
            simulation,
            sim_args: Vec::new(),
            lib_args: Vec::new(),
            delay: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
        })
    }

    /// The arguments of the simulation (`sim_args`), which the library reads in `Cosim_setup`.
    #[track_caller]
    pub fn with_sim_args<S: Into<String>>(
        mut self: Box<Self>,
        args: impl IntoIterator<Item = S>,
    ) -> Box<Self> {
        self.sim_args = string_args("simulation argument", args);
        self
    }

    /// The arguments of the library's shim (`lib_args`).
    #[track_caller]
    pub fn with_lib_args<S: Into<String>>(
        mut self: Box<Self>,
        args: impl IntoIterator<Item = S>,
    ) -> Box<Self> {
        self.lib_args = string_args("library argument", args);
        self
    }

    /// The delay of the outputs of the model (`delay`; ngspice's default is 1 ns).
    ///
    /// # Panics
    /// If `delay` is not positive.
    #[track_caller]
    pub fn with_delay(mut self: Box<Self>, delay: f64) -> Box<Self> {
        assert!(
            delay.is_finite() && delay > 0.0,
            "the delay must be positive: {delay}"
        );
        self.delay = Some(delay);
        self
    }

    /// Adds an input pin: the next input port of the model.
    ///
    /// # Panics
    /// If the name is not made of ASCII letters, digits and `_`, is `gnd`, `ref`, `0` or another
    /// pin's name (case-insensitively), or the characteristics are invalid.
    #[track_caller]
    pub fn input(mut self: Box<Self>, pin: &str, input: InputCharacteristics) -> Box<Self> {
        self.check_pin(pin);
        input.validate();
        self.inputs.push((pin.to_string(), input));
        self
    }

    /// Adds an output pin: the next output port of the model.
    ///
    /// # Panics
    /// As [`DCosim::input`].
    #[track_caller]
    pub fn output(mut self: Box<Self>, pin: &str, output: OutputCharacteristics) -> Box<Self> {
        self.check_pin(pin);
        output.validate();
        self.outputs.push((pin.to_string(), output));
        self
    }

    #[track_caller]
    fn check_pin(&self, pin: &str) {
        assert_subcircuit_pin(pin, self.pin_names(), "the part");
    }

    /// The subcircuit name and definition ([`hashed_subcircuit`]).
    fn subcircuit(&self) -> (String, String) {
        let mut body = String::new();
        for (pin, input) in &self.inputs {
            let (analog, digital) = (internal_node(pin, "ai"), internal_node(pin, "di"));
            body.push_str(&format!(
                "EI_{pin} {analog} 0 {pin} {SUBCIRCUIT_REFERENCE} 1\n\
                 AI_{pin} [{analog}] [{digital}] adc_{pin}\n\
                 .model adc_{pin} adc_bridge( in_low={} in_high={} )\n",
                spice_number(input.threshold_low),
                spice_number(input.threshold_high)
            ));
            body.push_str(&input.load_lines(pin));
        }
        let ports = |role: &str, pins: &mut dyn Iterator<Item = &String>| {
            let nodes: Vec<String> = pins.map(|pin| internal_node(pin, role)).collect();
            if nodes.is_empty() {
                "NULL".to_string()
            } else {
                format!("[{}]", nodes.join(" "))
            }
        };
        body.push_str(&format!(
            "AC {} {} cosim\n",
            ports("di", &mut self.inputs.iter().map(|p| &p.0)),
            ports("do", &mut self.outputs.iter().map(|p| &p.0))
        ));
        let strings = |values: &[String]| {
            let quoted: Vec<String> = values.iter().map(|v| format!("\"{v}\"")).collect();
            format!("[{}]", quoted.join(" "))
        };
        let mut model = format!(
            ".model cosim d_cosim( simulation=\"{}\"",
            self.simulation.display()
        );
        if !self.sim_args.is_empty() {
            model.push_str(&format!(" sim_args={}", strings(&self.sim_args)));
        }
        if !self.lib_args.is_empty() {
            model.push_str(&format!(" lib_args={}", strings(&self.lib_args)));
        }
        if let Some(delay) = self.delay {
            model.push_str(&format!(" delay={}", spice_number(delay)));
        }
        body.push_str(&model);
        body.push_str(" )\n");
        for (pin, output) in &self.outputs {
            let (digital, analog, drive) = (
                internal_node(pin, "do"),
                internal_node(pin, "ao"),
                internal_node(pin, "d"),
            );
            body.push_str(&format!(
                "AO_{pin} [{digital}] [{analog}] dac_{pin}\n\
                 .model dac_{pin} dac_bridge( out_low={} out_high={} out_undef={} t_rise={} t_fall={} )\n\
                 EO_{pin} {drive} {SUBCIRCUIT_REFERENCE} {analog} 0 1\n\
                 RO_{pin} {drive} {pin} {}\n",
                spice_number(output.low),
                spice_number(output.high),
                spice_number((output.low + output.high) / 2.0),
                spice_number(output.rise_time),
                spice_number(output.fall_time),
                spice_number(output.resistance)
            ));
        }
        hashed_subcircuit("dcosim", &self.pin_names(), &body)
    }

    fn pin_names(&self) -> Vec<&str> {
        self.inputs
            .iter()
            .map(|p| p.0.as_str())
            .chain(self.outputs.iter().map(|p| p.0.as_str()))
            .collect()
    }
}

impl Part for DCosim {
    fn get_ref_base_name(&self) -> String {
        "U".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        let mut pins: Vec<String> = self.pin_names().into_iter().map(Into::into).collect();
        pins.push(Self::GND.into());
        pins
    }
    fn value(&self) -> String {
        "d_cosim".into()
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        let (name, text) = self.subcircuit();
        Some(subcircuit_part(self.pin_ids(), name, text))
    }
}

mod verilator;

pub use verilator::{DEFAULT_SHIM_DIR, Port, VerilatedModel, Verilator, VerilatorError};

#[cfg(test)]
mod tests;
