//! Simulation-only parts for digital stimuli: a driver that plays a sequence of logic levels
//! through the electrical model of an output pin, and the load of an input pin.
//!
//! Like the parts of [`sources`](crate::spice::sources), they have no KiCad information and belong
//! in a test bench.
//!
//! ```
//! use cuprite_wire::spice::digital::{
//!     DigitalDriver, DigitalLoad, DigitalSequence, InputCharacteristics, Level,
//!     OutputCharacteristics,
//! };
//!
//! // "Hi" over a 115200 baud UART line, starting at 10 us.
//! let tx = DigitalSequence::new(Level::High).uart(10e-6, 115_200.0, b"Hi");
//! let driver = DigitalDriver::new(OutputCharacteristics::cmos(3.3), tx);
//! let load = DigitalLoad::new(InputCharacteristics::cmos(3.3));
//! ```

mod characteristics;
mod sequence;

#[cfg(feature = "shared-ngspice")]
pub(crate) use characteristics::StageSource;
pub use characteristics::{InputCharacteristics, OutputCharacteristics, Pull};
pub use sequence::DigitalSequence;
#[cfg(feature = "shared-ngspice")]
pub(crate) use sequence::Pwl;

use crate::{
    parts::Part,
    spice::{
        SpicePartInfo,
        subcircuit::{hashed_subcircuit, subcircuit_part},
    },
};

/// The resistance of an output in the high-impedance state.
pub const HIZ_RESISTANCE: f64 = 1e9;

/// A logic level driven by an output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    /// Driven to the low voltage ([`OutputCharacteristics::low`]).
    Low,
    /// Driven to the high voltage ([`OutputCharacteristics::high`]).
    High,
    /// High impedance: the output is disconnected (an open-drain output that is not pulling low,
    /// a tri-state output that is disabled).
    HiZ,
}

impl From<bool> for Level {
    fn from(high: bool) -> Self {
        if high { Level::High } else { Level::Low }
    }
}

/// An output pin playing a [`DigitalSequence`], with pins `out` and `gnd`.
///
/// It is written as a subcircuit of its own (`XDRV1 out gnd cuprite_digital_<hash>`), holding
/// the piecewise linear source of the voltage, the output resistance and, if the sequence has
/// `HiZ`, a voltage-controlled switch.
#[derive(Debug, Clone, PartialEq)]
pub struct DigitalDriver {
    output: OutputCharacteristics,
    sequence: DigitalSequence,
}

impl DigitalDriver {
    /// The output pin.
    pub const OUT: &'static str = "out";
    /// The reference pin the output voltages are relative to.
    pub const GND: &'static str = "gnd";

    /// # Panics
    /// If the levels of `output` are equal, or its resistance or an edge time is not positive.
    #[track_caller]
    pub fn new(output: OutputCharacteristics, sequence: DigitalSequence) -> Box<Self> {
        output.validate();
        Box::new(DigitalDriver { output, sequence })
    }

    /// The electrical characteristics of the output.
    pub fn output(&self) -> &OutputCharacteristics {
        &self.output
    }

    /// The levels the output plays.
    pub fn sequence(&self) -> &DigitalSequence {
        &self.sequence
    }

    /// The subcircuit name and definition ([`hashed_subcircuit`]).
    fn subcircuit(&self) -> (String, String) {
        let output = &self.output;
        let enable = self.sequence.enable_waveform(output.enable_transition());
        let body = output.stage_lines(
            Self::OUT,
            'V',
            &self.sequence.voltage_waveform(output).to_spice(),
            enable.map(|enable| enable.to_spice()).as_deref(),
        );
        hashed_subcircuit("digital", &[Self::OUT], &body)
    }
}

impl Part for DigitalDriver {
    fn get_ref_base_name(&self) -> String {
        "DRV".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        vec![Self::OUT.into(), Self::GND.into()]
    }
    fn value(&self) -> String {
        "DigitalDriver".into()
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        let (name, text) = self.subcircuit();
        Some(subcircuit_part([Self::OUT, Self::GND], name, text))
    }
}

/// The load of an input pin, with pins `in` and `gnd`: its capacitance, pull resistor and leakage.
///
/// The pull resistor and the leakage are written as their Norton equivalent: a resistor from `in`
/// to `gnd` (`RDL1P`) and a current source into `in` (`IDL1I`) of `voltage / resistance - leakage`.
/// The capacitor is `CDL1C`.
#[derive(Debug, Clone, PartialEq)]
pub struct DigitalLoad {
    input: InputCharacteristics,
}

impl DigitalLoad {
    /// The input pin.
    pub const IN: &'static str = "in";
    /// The reference pin the voltages of the input are relative to.
    pub const GND: &'static str = "gnd";

    /// # Panics
    /// If the thresholds of `input` cross, its capacitance is negative, its leakage is not
    /// finite, or its pull resistance is not positive.
    #[track_caller]
    pub fn new(input: InputCharacteristics) -> Box<Self> {
        input.validate();
        Box::new(DigitalLoad { input })
    }

    /// The electrical characteristics of the input.
    pub fn input(&self) -> &InputCharacteristics {
        &self.input
    }
}

impl Part for DigitalLoad {
    fn get_ref_base_name(&self) -> String {
        "DL".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        vec![Self::IN.into(), Self::GND.into()]
    }
    fn value(&self) -> String {
        "DigitalLoad".into()
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        let elements = self.input.load_elements(Self::IN, Self::GND);
        Some(if elements.is_empty() {
            // An ideal input draws nothing.
            SpicePartInfo::Excluded
        } else {
            SpicePartInfo::elements(elements)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spice::sources::spice_number;

    #[test]
    fn test_driver_subcircuit() {
        let push_pull = DigitalDriver::new(
            OutputCharacteristics::cmos(3.3),
            DigitalSequence::new(Level::Low).at(1e-6, Level::High),
        );
        let (name, text) = push_pull.subcircuit();
        assert!(name.starts_with("cuprite_digital_"), "{name}");
        let edge_end = spice_number(1e-6 + 2e-9);
        assert_eq!(
            text,
            format!(
                ".subckt {name} out ref\n\
                 VD_out out#d ref PWL( 0 0 1e-6 0 {edge_end} 3.3 )\n\
                 RO_out out#d out 50\n\
                 .ends {name}\n"
            )
        );
        let open_drain = DigitalDriver::new(
            OutputCharacteristics::cmos(3.3),
            DigitalSequence::new(Level::Low).at(1e-6, Level::HiZ),
        );
        let (other, text) = open_drain.subcircuit();
        assert_ne!(other, name);
        assert!(
            text.contains(&format!(
                "VE_out out#e ref PWL( 0 1 1e-6 1 {edge_end} 0 )\n\
                     SO_out out#d out out#e ref hiz_out\n\
                     .model hiz_out SW( vt=0.5 vh=0 ron=50 roff=1e9 )\n"
            )),
            "{text}"
        );
    }

    #[test]
    #[should_panic(expected = "the output rise time must be positive")]
    fn test_driver_ideal_edge() {
        let _ = DigitalDriver::new(
            OutputCharacteristics::cmos(3.3).with_edges(0.0, 1e-9),
            DigitalSequence::new(Level::Low),
        );
    }

    #[test]
    #[should_panic(expected = "the input thresholds must be finite")]
    fn test_load_infinite_threshold() {
        let input = InputCharacteristics {
            threshold_high: f64::INFINITY,
            ..InputCharacteristics::cmos(3.3)
        };
        let _ = DigitalLoad::new(input);
    }

    #[test]
    fn test_load_elements() {
        let elements = |input: InputCharacteristics| match DigitalLoad::new(input).spice() {
            Some(SpicePartInfo::Elements { elements, .. }) => elements
                .iter()
                .map(|e| format!("{}{} {}", e.device, e.suffix, e.args))
                .collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        let input = InputCharacteristics::cmos(3.3).with_pull(Pull::up(3.3, 10e3));
        assert_eq!(
            elements(input.clone()),
            ["CC 5e-12", "RP 10000", "II DC 3.3e-4"]
        );
        assert_eq!(
            elements(input.with_pull(Pull::down(1e3)).with_leakage(1e-6)),
            ["CC 5e-12", "RP 1000", "II DC -1e-6"]
        );
        let ideal = InputCharacteristics::cmos(3.3).with_capacitance(0.0);
        assert_eq!(
            DigitalLoad::new(ideal).spice(),
            Some(SpicePartInfo::Excluded)
        );
    }
}
