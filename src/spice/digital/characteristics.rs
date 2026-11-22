//! The electrical characteristics of output and input pins.

use super::{DigitalSequence, HIZ_RESISTANCE, Level};
use crate::spice::{
    SpiceElement,
    netlist::write_element_line,
    spice_number,
    subcircuit::{SUBCIRCUIT_REFERENCE, internal_node},
};

/// A source of the output stage of a pin ([`OutputCharacteristics::stage_lines`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StageSource {
    /// The voltage of the output, at the node `<pin>#d`.
    Drive,
    /// Above 0.5 V while the output is driven, below it in high impedance, at `<pin>#e`.
    Enable,
}

impl StageSource {
    fn role(self) -> &'static str {
        match self {
            StageSource::Drive => "d",
            StageSource::Enable => "e",
        }
    }

    /// The node of the source: `<pin>#d` or `<pin>#e`.
    pub(crate) fn node(self, pin: &str) -> String {
        internal_node(pin, self.role())
    }

    /// The name of the source, an element of the device `device`: e.g. `VD_<pin>`, `VE_<pin>`.
    pub(crate) fn element(self, device: char, pin: &str) -> String {
        format!("{device}{}_{pin}", self.role().to_ascii_uppercase())
    }
}

/// The electrical characteristics of an output pin.
///
/// The output is a voltage source behind `resistance`. Edges are linear from one level to the
/// other in `rise_time` / `fall_time` (0-100 %, which is 1.25 times the 10-90 % time of a datasheet).
/// Changes to and from `HiZ` switch the resistance to [`HIZ_RESISTANCE`] and back.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct OutputCharacteristics {
    /// The voltage of `Low` (`V_OL`), relative to the `gnd` pin.
    pub low: f64,
    /// The voltage of `High` (`V_OH`), relative to the `gnd` pin.
    pub high: f64,
    /// The output resistance in ohms.
    pub resistance: f64,
    /// The time in seconds of a full edge from `low` to `high` (0-100 %).
    pub rise_time: f64,
    /// The time in seconds of a full edge from `high` to `low` (0-100 %).
    pub fall_time: f64,
}

impl OutputCharacteristics {
    /// A push-pull CMOS output switching between 0 V and `vdd`, with 50 ohm and 2 ns edges.
    /// Take the values of the actual device from its datasheet.
    pub fn cmos(vdd: f64) -> Self {
        OutputCharacteristics {
            low: 0.0,
            high: vdd,
            resistance: 50.0,
            rise_time: 2e-9,
            fall_time: 2e-9,
        }
    }

    /// Sets the output resistance in ohms.
    pub fn with_resistance(mut self, resistance: f64) -> Self {
        self.resistance = resistance;
        self
    }

    /// Sets the full (0-100 %) rise and fall times in seconds.
    pub fn with_edges(mut self, rise_time: f64, fall_time: f64) -> Self {
        self.rise_time = rise_time;
        self.fall_time = fall_time;
        self
    }

    /// The time an edge from `from` to `to` volts takes: the rise or fall time, scaled by the
    /// part of the swing it covers.
    pub(crate) fn edge_duration(&self, from: f64, to: f64) -> f64 {
        let edge = if to > from {
            self.rise_time
        } else {
            self.fall_time
        };
        edge * (to - from).abs() / (self.high - self.low).abs()
    }

    /// The time the switch to and from `HiZ` takes (the shorter edge).
    pub(crate) fn enable_transition(&self) -> f64 {
        self.rise_time.min(self.fall_time)
    }

    /// The lines of the output stage of the pin `pin` of a generated subcircuit: the drive
    /// source `<device>D_<pin>` from [`SUBCIRCUIT_REFERENCE`] to the node `<pin>#d`, with the
    /// arguments `drive` after its nodes, behind the output resistance `RO_<pin>`. With `enable`
    /// (the arguments of the voltage source `VE_<pin>` at `<pin>#e`), the resistance is the switch
    /// `SO_<pin>` (model `hiz_<pin>`) instead, closed while `VE_<pin>` is above 0.5 V and at
    /// [`HIZ_RESISTANCE`] otherwise.
    pub(crate) fn stage_lines(
        &self,
        pin: &str,
        device: char,
        drive: &str,
        enable: Option<&str>,
    ) -> String {
        let drive_node = StageSource::Drive.node(pin);
        let mut text = format!(
            "{} {drive_node} {SUBCIRCUIT_REFERENCE} {drive}\n",
            StageSource::Drive.element(device, pin)
        );
        let resistance = spice_number(self.resistance);
        match enable {
            None => text.push_str(&format!("RO_{pin} {drive_node} {pin} {resistance}\n")),
            Some(enable) => {
                let enable_node = StageSource::Enable.node(pin);
                text.push_str(&format!(
                    "{} {enable_node} {SUBCIRCUIT_REFERENCE} {enable}\n\
                     SO_{pin} {drive_node} {pin} {enable_node} {SUBCIRCUIT_REFERENCE} hiz_{pin}\n\
                     .model hiz_{pin} SW( vt=0.5 vh=0 ron={resistance} roff={} )\n",
                    StageSource::Enable.element('V', pin),
                    spice_number(HIZ_RESISTANCE)
                ));
            }
        }
        text
    }

    #[track_caller]
    pub(crate) fn validate(&self) {
        assert!(
            self.low.is_finite() && self.high.is_finite() && self.low != self.high,
            "the output levels must be finite and differ: {} V and {} V",
            self.low,
            self.high
        );
        for (name, value) in [
            ("resistance", self.resistance),
            ("rise time", self.rise_time),
            ("fall time", self.fall_time),
        ] {
            // Ideal steps and sources make the simulation fail to converge.
            assert!(
                value.is_finite() && value > 0.0,
                "the output {name} must be positive: {value}"
            );
        }
    }
}

/// A pull-up or pull-down resistor of an input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pull {
    /// The voltage the resistor pulls to, relative to the `gnd` pin.
    pub voltage: f64,
    /// The resistance in ohms.
    pub resistance: f64,
}

impl Pull {
    /// A resistor of `resistance` ohms pulling to `voltage` volts.
    pub fn up(voltage: f64, resistance: f64) -> Self {
        Pull {
            voltage,
            resistance,
        }
    }

    /// A resistor of `resistance` ohms pulling to the `gnd` pin (0 V).
    pub fn down(resistance: f64) -> Self {
        Pull::up(0.0, resistance)
    }
}

/// The electrical characteristics of an input pin.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct InputCharacteristics {
    /// The highest voltage read as `Low` (`V_IL`).
    pub threshold_low: f64,
    /// The lowest voltage read as `High` (`V_IH`). Between the thresholds, the input keeps the
    /// level it has read (hysteresis).
    pub threshold_high: f64,
    /// The input capacitance in farads.
    pub capacitance: f64,
    /// The pull-up or pull-down resistor, if any.
    pub pull: Option<Pull>,
    /// The current flowing into the pin, in amperes.
    pub leakage: f64,
}

impl InputCharacteristics {
    /// A CMOS input with thresholds at 30 % and 70 % of `vdd` and 5 pF, without pull resistor
    /// or leakage. Take the values of the actual device from its datasheet.
    pub fn cmos(vdd: f64) -> Self {
        InputCharacteristics {
            threshold_low: 0.3 * vdd,
            threshold_high: 0.7 * vdd,
            capacitance: 5e-12,
            pull: None,
            leakage: 0.0,
        }
    }

    /// Sets the input capacitance in farads.
    pub fn with_capacitance(mut self, capacitance: f64) -> Self {
        self.capacitance = capacitance;
        self
    }

    /// Adds a pull-up or pull-down resistor.
    pub fn with_pull(mut self, pull: Pull) -> Self {
        self.pull = Some(pull);
        self
    }

    /// Sets the current flowing into the pin, in amperes.
    pub fn with_leakage(mut self, leakage: f64) -> Self {
        self.leakage = leakage;
        self
    }

    /// The level read from `voltage` by an input that has read `previous`.
    pub fn read(&self, voltage: f64, previous: bool) -> bool {
        if voltage >= self.threshold_high {
            true
        } else if voltage <= self.threshold_low {
            false
        } else {
            previous
        }
    }

    /// The level read from `voltage` by an input that has not read before: between the
    /// thresholds, the level of the nearer one.
    pub fn read_initial(&self, voltage: f64) -> bool {
        self.read(
            voltage,
            voltage >= (self.threshold_low + self.threshold_high) / 2.0,
        )
    }

    /// The levels read from voltage samples (e.g. columns of a transient analysis [`Table`]).
    /// The first sample sets the initial level (between the thresholds, the nearer one);
    /// the times of the changes are interpolated at the threshold crossed.
    ///
    /// [`Table`]: crate::spice::Table
    ///
    /// # Panics
    /// If `time` and `voltage` have different lengths or the times decrease.
    #[track_caller]
    pub fn decode(&self, time: &[f64], voltage: &[f64]) -> DigitalSequence {
        assert_eq!(
            time.len(),
            voltage.len(),
            "the samples must have one time each"
        );
        let Some(&first) = voltage.first() else {
            return DigitalSequence::new(Level::Low);
        };
        let mut high = self.read_initial(first);
        let mut sequence = DigitalSequence::new(Level::from(high));
        for i in 1..time.len() {
            let now = self.read(voltage[i], high);
            if now != high {
                let threshold = if now {
                    self.threshold_high
                } else {
                    self.threshold_low
                };
                let (t0, v0, t1, v1) = (time[i - 1], voltage[i - 1], time[i], voltage[i]);
                let crossing = t0 + (t1 - t0) * (threshold - v0) / (v1 - v0);
                sequence.push(crossing.clamp(t0, t1), Level::from(now));
                high = now;
            }
        }
        sequence
    }

    /// The elements of the load between the pins `pin` and `gnd`: the capacitor (suffix `C`),
    /// and the pull resistor and the leakage as their Norton equivalent (`P`, `I`).
    pub(crate) fn load_elements(&self, pin: &str, gnd: &str) -> Vec<SpiceElement> {
        let mut elements = Vec::new();
        if self.capacitance > 0.0 {
            elements.push(
                SpiceElement::new('C', [pin, gnd], spice_number(self.capacitance)).with_suffix("C"),
            );
        }
        let mut current = -self.leakage;
        if let Some(pull) = self.pull {
            elements.push(
                SpiceElement::new('R', [pin, gnd], spice_number(pull.resistance)).with_suffix("P"),
            );
            current += pull.voltage / pull.resistance;
        }
        if current != 0.0 {
            elements.push(
                SpiceElement::new('I', [gnd, pin], format!("DC {}", spice_number(current)))
                    .with_suffix("I"),
            );
        }
        elements
    }

    /// The lines of the load ([`InputCharacteristics::load_elements`]) of the pin `pin` of a
    /// generated subcircuit, against [`SUBCIRCUIT_REFERENCE`]: `CC_<pin>`, `RP_<pin>`, `II_<pin>`.
    pub(crate) fn load_lines(&self, pin: &str) -> String {
        let mut text = String::new();
        for element in self.load_elements(pin, SUBCIRCUIT_REFERENCE) {
            let name = format!("{}{}_{pin}", element.device, element.suffix);
            write_element_line(&mut text, &name, &element.nodes, &element.args)
                .expect("writing to a String does not fail");
        }
        text
    }

    #[track_caller]
    pub(crate) fn validate(&self) {
        assert!(
            self.threshold_low.is_finite()
                && self.threshold_high.is_finite()
                && self.threshold_low <= self.threshold_high,
            "the input thresholds must be finite and not cross: V_IL {} V, V_IH {} V",
            self.threshold_low,
            self.threshold_high
        );
        assert!(
            self.capacitance.is_finite() && self.capacitance >= 0.0,
            "the input capacitance must not be negative: {}",
            self.capacitance
        );
        assert!(
            self.leakage.is_finite(),
            "the input leakage must be finite: {}",
            self.leakage
        );
        if let Some(pull) = self.pull {
            assert!(
                pull.voltage.is_finite() && pull.resistance.is_finite() && pull.resistance > 0.0,
                "the pull resistor must be positive and pull to a finite voltage: {pull:?}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode() {
        let input = InputCharacteristics::cmos(1.0);
        let time = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
        // Hysteresis: 0.5 V keeps the level; the crossings are interpolated.
        let voltage = [0.5, 0.9, 0.5, 0.0, 0.5, 0.5];
        let sequence = input.decode(&time, &voltage);
        assert_eq!(sequence.initial(), Level::High);
        assert_eq!(sequence.events().len(), 1);
        let (t, level) = sequence.events()[0];
        assert_eq!(level, Level::Low);
        // From 0.5 V at 2 s to 0 V at 3 s, crossing 0.3 V at 2.4 s.
        assert!((t - 2.4).abs() < 1e-12, "{t}");
        assert!(!input.read(0.4, false) && input.read(0.4, true));
    }

    #[test]
    fn test_load_lines() {
        let input = InputCharacteristics::cmos(3.3).with_pull(Pull::up(3.3, 10e3));
        assert_eq!(
            input.load_lines("p"),
            "CC_p p ref 5e-12\nRP_p p ref 10000\nII_p ref p DC 3.3e-4\n"
        );
    }

    #[test]
    fn test_read_initial() {
        let input = InputCharacteristics::cmos(10.0);
        assert!(!input.read_initial(4.9));
        assert!(input.read_initial(5.0));
        assert!(!input.read_initial(0.0));
        assert!(input.read_initial(8.0));
    }
}
