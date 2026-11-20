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
        ModelSource, SpiceElement, SpicePartInfo,
        digital::{InputCharacteristics, OutputCharacteristics, SUBCIRCUIT_REFERENCE, fnv1a},
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
/// sources relative to `ref`.
#[derive(Debug, Clone, PartialEq)]
pub struct DCosim {
    simulation: PathBuf,
    sim_args: Vec<String>,
    lib_args: Vec<String>,
    delay: Option<f64>,
    inputs: Vec<(String, InputCharacteristics)>,
    outputs: Vec<(String, OutputCharacteristics)>,
}

/// Names written into the subcircuit: ASCII letters, digits and `_`.
fn is_identifier(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// An argument written as a quoted string of the model.
#[track_caller]
fn check_string(what: &str, value: &str) {
    assert!(
        !value.contains(|c: char| c == '"' || c.is_control()),
        "the {what} {value:?} cannot be written in a model (it contains '\"' or a control character)"
    );
}

impl DCosim {
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
        self.sim_args = args.into_iter().map(Into::into).collect();
        for arg in &self.sim_args {
            check_string("simulation argument", arg);
        }
        self
    }

    /// The arguments of the library's shim (`lib_args`).
    #[track_caller]
    pub fn with_lib_args<S: Into<String>>(
        mut self: Box<Self>,
        args: impl IntoIterator<Item = S>,
    ) -> Box<Self> {
        self.lib_args = args.into_iter().map(Into::into).collect();
        for arg in &self.lib_args {
            check_string("library argument", arg);
        }
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
    /// If the name is not made of ASCII letters, digits and `_`, is `gnd` or another pin's name
    /// (case-insensitively), or the characteristics are invalid.
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
        assert!(
            is_identifier(pin),
            "a pin name may only contain ASCII letters, digits and '_': {pin:?}"
        );
        let names = self
            .inputs
            .iter()
            .map(|p| &p.0)
            .chain(self.outputs.iter().map(|p| &p.0));
        assert!(
            !pin.eq_ignore_ascii_case(Self::GND)
                && !names.into_iter().any(|name| name.eq_ignore_ascii_case(pin)),
            "the part already has a pin {pin:?}"
        );
    }

    /// The subcircuit name and definition. The name is derived from the definition, so equal
    /// parts share one definition.
    fn subcircuit(&self) -> (String, String) {
        let mut body = String::new();
        for (pin, input) in &self.inputs {
            body.push_str(&format!(
                "EI_{pin} ai_{pin} 0 {pin} {SUBCIRCUIT_REFERENCE} 1\n\
                 AI_{pin} [ai_{pin}] [di_{pin}] adc_{pin}\n\
                 .model adc_{pin} adc_bridge( in_low={} in_high={} )\n",
                spice_number(input.threshold_low),
                spice_number(input.threshold_high)
            ));
            for element in input.load_elements(pin, SUBCIRCUIT_REFERENCE) {
                body.push_str(&format!(
                    "{}{}_{pin} {} {}\n",
                    element.device,
                    element.suffix,
                    element.nodes.join(" "),
                    element.args
                ));
            }
        }
        let ports = |prefix: &str, pins: &mut dyn Iterator<Item = &String>| {
            let nodes: Vec<String> = pins.map(|pin| format!("{prefix}_{pin}")).collect();
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
            body.push_str(&format!(
                "AO_{pin} [do_{pin}] [ao_{pin}] dac_{pin}\n\
                 .model dac_{pin} dac_bridge( out_low={} out_high={} out_undef={} t_rise={} t_fall={} )\n\
                 EO_{pin} d_{pin} {SUBCIRCUIT_REFERENCE} ao_{pin} 0 1\n\
                 RO_{pin} d_{pin} {pin} {}\n",
                spice_number(output.low),
                spice_number(output.high),
                spice_number((output.low + output.high) / 2.0),
                spice_number(output.rise_time),
                spice_number(output.fall_time),
                spice_number(output.resistance)
            ));
        }
        let name = format!("cuprite_dcosim_{:016x}", fnv1a(body.as_bytes()));
        let text = format!(
            ".subckt {name} {} {SUBCIRCUIT_REFERENCE}\n{body}.ends {name}\n",
            self.pin_names().join(" ")
        );
        (name, text)
    }

    fn pin_names(&self) -> Vec<String> {
        self.inputs
            .iter()
            .map(|p| p.0.clone())
            .chain(self.outputs.iter().map(|p| p.0.clone()))
            .collect()
    }
}

impl Part for DCosim {
    fn get_ref_base_name(&self) -> String {
        "U".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        let mut pins = self.pin_names();
        pins.push(Self::GND.into());
        pins
    }
    fn value(&self) -> String {
        "d_cosim".into()
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        let (name, text) = self.subcircuit();
        Some(SpicePartInfo::element(
            SpiceElement::new('X', self.pin_ids(), name.clone())
                .with_model(ModelSource::inline(name, text)),
        ))
    }
}

mod verilator;

pub use verilator::{DEFAULT_SHIM_DIR, Port, VerilatedModel, Verilator, VerilatorError};

#[cfg(test)]
mod tests;
