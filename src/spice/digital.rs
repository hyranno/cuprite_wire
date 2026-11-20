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

use crate::{
    parts::Part,
    spice::{
        ModelSource, SpiceElement, SpicePartInfo,
        sources::{Waveform, spice_number},
    },
};

/// The node of the generated subcircuits that their `gnd` pin connects to. ngspice reads a node
/// named `gnd` as the ground (node 0), even as a port of a subcircuit, so it cannot be used there.
pub(crate) const SUBCIRCUIT_REFERENCE: &str = "ref";

/// The resistance of an output in the high-impedance state.
pub const HIZ_RESISTANCE: f64 = 1e9;

/// A logic level driven by an output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    Low,
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

/// A sequence of logic levels in time: an initial level and the times at which it changes.
#[derive(Debug, Clone, PartialEq)]
pub struct DigitalSequence {
    initial: Level,
    events: Vec<(f64, Level)>,
}

impl DigitalSequence {
    pub fn new(initial: Level) -> Self {
        DigitalSequence {
            initial,
            events: Vec::new(),
        }
    }

    /// Changes the level at `time` (in seconds). Times must not decrease; setting the level the
    /// sequence already has is ignored.
    ///
    /// # Panics
    /// If `time` is negative, not finite, or before the last change.
    #[track_caller]
    pub fn at(mut self, time: f64, level: Level) -> Self {
        self.push(time, level);
        self
    }

    /// Like [`DigitalSequence::at`], in place.
    #[track_caller]
    pub fn push(&mut self, time: f64, level: Level) {
        assert!(
            time.is_finite() && time >= 0.0,
            "the time of a level change must be finite and not negative: {time}"
        );
        assert!(
            time >= self.end_time(),
            "the level changes at {time} s, before the last change at {} s",
            self.end_time()
        );
        if level != self.final_level() {
            self.events.push((time, level));
        }
    }

    pub fn initial(&self) -> Level {
        self.initial
    }

    /// The changes as `(time, new level)`, in increasing time. Consecutive levels differ.
    pub fn events(&self) -> &[(f64, Level)] {
        &self.events
    }

    /// The level after the last change.
    pub fn final_level(&self) -> Level {
        self.events.last().map_or(self.initial, |&(_, level)| level)
    }

    /// The time of the last change (0 without changes).
    pub fn end_time(&self) -> f64 {
        self.events.last().map_or(0.0, |&(time, _)| time)
    }

    /// The level at `time` (a change at exactly `time` has taken effect).
    pub fn level_at(&self, time: f64) -> Level {
        self.events
            .iter()
            .take_while(|&&(t, _)| t <= time)
            .last()
            .map_or(self.initial, |&(_, level)| level)
    }

    /// Appends UART frames (8 data bits, LSB first, no parity, 1 stop bit) sent back to back from
    /// `start`, at `baud` bits per second. The line should be idle (`High`) before `start`.
    #[track_caller]
    pub fn uart(mut self, start: f64, baud: f64, bytes: &[u8]) -> Self {
        assert!(
            baud.is_finite() && baud > 0.0,
            "the baud rate must be positive: {baud}"
        );
        let bit = 1.0 / baud;
        for (i, byte) in bytes.iter().enumerate() {
            let frame = start + (i * 10) as f64 * bit;
            self.push(frame, Level::Low);
            for b in 0..8 {
                let time = frame + (1 + b) as f64 * bit;
                self.push(time, Level::from(byte >> b & 1 == 1));
            }
            self.push(frame + 9.0 * bit, Level::High);
        }
        self
    }

    /// Appends `cycles` periods of a PWM signal from `start`: `High` for `duty * period`, then
    /// `Low`. The sequence is `Low` at the end.
    #[track_caller]
    pub fn pwm(mut self, start: f64, period: f64, duty: f64, cycles: usize) -> Self {
        assert!(
            period.is_finite() && period > 0.0,
            "the PWM period must be positive: {period}"
        );
        assert!(
            (0.0..=1.0).contains(&duty),
            "the PWM duty must be in 0..=1: {duty}"
        );
        for k in 0..cycles {
            let begin = start + k as f64 * period;
            if duty > 0.0 {
                self.push(begin, Level::High);
            }
            if duty < 1.0 {
                self.push(begin + duty * period, Level::Low);
            }
        }
        self.push(start + cycles as f64 * period, Level::Low);
        self
    }

    /// The voltage of an ideal driver following the sequence with the levels and edges of
    /// `output`, starting at time 0. `HiZ` holds the last driven voltage (`low` if the sequence
    /// starts in `HiZ`); [`DigitalSequence::enable_waveform`] tells when the output is disconnected.
    ///
    /// # Panics
    /// If `output` is invalid (see [`DigitalDriver::new`]).
    #[track_caller]
    pub fn voltage_waveform(&self, output: &OutputCharacteristics) -> Waveform {
        output.validate();
        let voltage = |level| match level {
            Level::Low => Some(output.low),
            Level::High => Some(output.high),
            Level::HiZ => None,
        };
        let mut pwl = Pwl::new(voltage(self.initial).unwrap_or(output.low));
        for &(time, level) in &self.events {
            if let Some(target) = voltage(level) {
                pwl.ramp_to(time, target, |from, to| output.edge_duration(from, to));
            }
        }
        Waveform::Pwl(pwl.points)
    }

    /// 1 while the output drives the line and 0 while it is `HiZ`, changing linearly over
    /// `transition` seconds; `None` if the sequence is never `HiZ`.
    ///
    /// # Panics
    /// If `transition` is not positive.
    #[track_caller]
    pub fn enable_waveform(&self, transition: f64) -> Option<Waveform> {
        assert!(
            transition.is_finite() && transition > 0.0,
            "the transition time must be positive: {transition}"
        );
        let enable = |level| if level == Level::HiZ { 0.0 } else { 1.0 };
        let levels = || std::iter::once(self.initial).chain(self.events.iter().map(|e| e.1));
        if !levels().any(|level| level == Level::HiZ) {
            return None;
        }
        let mut pwl = Pwl::new(enable(self.initial));
        for &(time, level) in &self.events {
            pwl.ramp_to(time, enable(level), |from, to| {
                transition * (to - from).abs()
            });
        }
        Some(Waveform::Pwl(pwl.points))
    }
}

/// Points of a piecewise linear waveform built from ramps.
#[derive(Debug, Clone)]
pub(crate) struct Pwl {
    pub(crate) points: Vec<(f64, f64)>,
}

impl Pwl {
    pub(crate) fn new(initial: f64) -> Self {
        Pwl {
            points: vec![(0.0, initial)],
        }
    }

    /// The value at `time`, which is not before the start of the last ramp.
    pub(crate) fn value_at(&self, time: f64) -> f64 {
        match self.points[..] {
            [.., (t0, v0), (t1, v1)] if time < t1 => {
                if time <= t0 {
                    v0
                } else {
                    v0 + (v1 - v0) * (time - t0) / (t1 - t0)
                }
            }
            [.., (_, v)] => v,
            [] => unreachable!("a waveform starts with a point"),
        }
    }

    /// Ramps from the value at `time` to `target` in `duration(from, target)` seconds,
    /// cutting the previous ramp short if it has not ended yet.
    pub(crate) fn ramp_to(&mut self, time: f64, target: f64, duration: impl Fn(f64, f64) -> f64) {
        if self.points.last().is_some_and(|&(_, v)| v == target) {
            // Already there, or ramping there.
            return;
        }
        let current = self.value_at(time);
        while self.points.last().is_some_and(|&(t, _)| t > time) {
            self.points.pop();
        }
        if self.points.last().is_some_and(|&(t, _)| t < time) {
            self.points.push((time, current));
        }
        self.points.push((time + duration(current, target), target));
    }
}

/// The electrical characteristics of an output pin.
///
/// The output is a voltage source behind `resistance`. Edges are linear from one level to the
/// other in `rise_time` / `fall_time` (0-100 %, which is 1.25 times the 10-90 % time of a datasheet).
/// Changes to and from `HiZ` switch the resistance to [`HIZ_RESISTANCE`] and back.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputCharacteristics {
    /// The voltage of `Low` (`V_OL`), relative to the `gnd` pin.
    pub low: f64,
    /// The voltage of `High` (`V_OH`), relative to the `gnd` pin.
    pub high: f64,
    /// The output resistance in ohms.
    pub resistance: f64,
    pub rise_time: f64,
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

    pub fn with_resistance(mut self, resistance: f64) -> Self {
        self.resistance = resistance;
        self
    }

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

    /// The lines of the high-impedance switch from the source node `drive` to `out`,
    /// closed while the voltage of `enable` above [`SUBCIRCUIT_REFERENCE`] is above 0.5 V:
    /// an `S` element `SO<suffix>` and its model `hiz<suffix>`.
    pub(crate) fn switch_lines(
        &self,
        suffix: &str,
        drive: &str,
        out: &str,
        enable: &str,
    ) -> String {
        format!(
            "SO{suffix} {drive} {out} {enable} {SUBCIRCUIT_REFERENCE} hiz{suffix}\n\
             .model hiz{suffix} SW( vt=0.5 vh=0 ron={} roff={} )\n",
            spice_number(self.resistance),
            spice_number(HIZ_RESISTANCE)
        )
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
    pub resistance: f64,
}

impl Pull {
    pub fn up(voltage: f64, resistance: f64) -> Self {
        Pull {
            voltage,
            resistance,
        }
    }

    pub fn down(resistance: f64) -> Self {
        Pull::up(0.0, resistance)
    }
}

/// The electrical characteristics of an input pin.
#[derive(Debug, Clone, PartialEq)]
pub struct InputCharacteristics {
    /// The highest voltage read as `Low` (`V_IL`).
    pub threshold_low: f64,
    /// The lowest voltage read as `High` (`V_IH`). Between the thresholds, the input keeps the
    /// level it has read (hysteresis).
    pub threshold_high: f64,
    /// The input capacitance in farads.
    pub capacitance: f64,
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

    pub fn with_capacitance(mut self, capacitance: f64) -> Self {
        self.capacitance = capacitance;
        self
    }

    pub fn with_pull(mut self, pull: Pull) -> Self {
        self.pull = Some(pull);
        self
    }

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
        let middle = (self.threshold_low + self.threshold_high) / 2.0;
        let mut high = self.read(first, first >= middle);
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

    #[track_caller]
    pub(crate) fn validate(&self) {
        assert!(
            self.threshold_low <= self.threshold_high,
            "the input thresholds must not cross: V_IL {} V, V_IH {} V",
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
    pub const OUT: &'static str = "out";
    pub const GND: &'static str = "gnd";

    /// # Panics
    /// If the levels of `output` are equal, or its resistance or an edge time is not positive.
    #[track_caller]
    pub fn new(output: OutputCharacteristics, sequence: DigitalSequence) -> Box<Self> {
        output.validate();
        Box::new(DigitalDriver { output, sequence })
    }

    pub fn output(&self) -> &OutputCharacteristics {
        &self.output
    }

    pub fn sequence(&self) -> &DigitalSequence {
        &self.sequence
    }

    /// The subcircuit name and definition. The name is derived from the definition, so equal
    /// drivers share one definition.
    fn subcircuit(&self) -> (String, String) {
        let output = &self.output;
        let mut body = format!(
            "VD d {SUBCIRCUIT_REFERENCE} {}\n",
            self.sequence.voltage_waveform(output).to_spice()
        );
        match self.sequence.enable_waveform(output.enable_transition()) {
            None => body.push_str(&format!("RO d out {}\n", spice_number(output.resistance))),
            Some(enable) => {
                body.push_str(&format!(
                    "VE e {SUBCIRCUIT_REFERENCE} {}\n",
                    enable.to_spice()
                ));
                body.push_str(&output.switch_lines("", "d", "out", "e"));
            }
        }
        let name = format!("cuprite_digital_{:016x}", fnv1a(body.as_bytes()));
        let text = format!(".subckt {name} out {SUBCIRCUIT_REFERENCE}\n{body}.ends {name}\n");
        (name, text)
    }
}

/// The 64-bit FNV-1a hash: stable across platforms and Rust versions, unlike `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
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
        Some(SpicePartInfo::element(
            SpiceElement::new('X', [Self::OUT, Self::GND], name.clone())
                .with_model(ModelSource::inline(name, text)),
        ))
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
    pub const IN: &'static str = "in";
    pub const GND: &'static str = "gnd";

    /// # Panics
    /// If the thresholds of `input` cross, its capacitance is negative, its leakage is not
    /// finite, or its pull resistance is not positive.
    #[track_caller]
    pub fn new(input: InputCharacteristics) -> Box<Self> {
        input.validate();
        Box::new(DigitalLoad { input })
    }

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

    fn pwl(waveform: Waveform) -> Vec<(f64, f64)> {
        match waveform {
            Waveform::Pwl(points) => points,
            other => panic!("not a PWL: {other:?}"),
        }
    }

    fn assert_points(actual: &[(f64, f64)], expected: &[(f64, f64)]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert!(
                (a.0 - e.0).abs() < 1e-15 && (a.1 - e.1).abs() < 1e-12,
                "{actual:?} != {expected:?}"
            );
        }
    }

    #[test]
    fn test_sequence() {
        let sequence = DigitalSequence::new(Level::Low)
            .at(1.0, Level::High)
            .at(1.5, Level::High)
            .at(2.0, Level::HiZ)
            .at(2.0, Level::Low);
        assert_eq!(
            sequence.events(),
            [(1.0, Level::High), (2.0, Level::HiZ), (2.0, Level::Low)]
        );
        assert_eq!(sequence.level_at(0.5), Level::Low);
        assert_eq!(sequence.level_at(1.0), Level::High);
        assert_eq!(sequence.level_at(3.0), Level::Low);
        assert_eq!(sequence.end_time(), 2.0);
    }

    #[test]
    #[should_panic(expected = "before the last change")]
    fn test_sequence_decreasing_time() {
        let _ = DigitalSequence::new(Level::Low)
            .at(2.0, Level::High)
            .at(1.0, Level::Low);
    }

    #[test]
    fn test_uart() {
        // 0x41 = 0b0100_0001: start, 1 0 0 0 0 0 1 0 (LSB first), stop.
        let sequence = DigitalSequence::new(Level::High).uart(1.0, 10.0, b"A");
        let bits: Vec<Level> = (0..10)
            .map(|i| sequence.level_at(1.0 + (i as f64 + 0.5) * 0.1))
            .collect();
        use Level::{High as H, Low as L};
        assert_eq!(bits, [L, H, L, L, L, L, L, H, L, H]);
        assert_eq!(sequence.final_level(), Level::High);
    }

    #[test]
    fn test_pwm() {
        let sequence = DigitalSequence::new(Level::Low).pwm(0.0, 4.0, 0.25, 2);
        assert_eq!(
            sequence.events(),
            [
                (0.0, Level::High),
                (1.0, Level::Low),
                (4.0, Level::High),
                (5.0, Level::Low)
            ]
        );
        let full = DigitalSequence::new(Level::Low).pwm(1.0, 1.0, 1.0, 3);
        assert_eq!(full.events(), [(1.0, Level::High), (4.0, Level::Low)]);
        let none = DigitalSequence::new(Level::Low).pwm(1.0, 1.0, 0.0, 3);
        assert!(none.events().is_empty());
    }

    #[test]
    fn test_voltage_waveform() {
        let output = OutputCharacteristics::cmos(2.0).with_edges(0.2, 0.4);
        let sequence = DigitalSequence::new(Level::Low)
            .at(0.0, Level::High)
            .at(1.0, Level::HiZ)
            .at(2.0, Level::Low)
            // Cut short at half of the rise: 1 V, falling back in 0.2 s.
            .at(3.0, Level::High)
            .at(3.1, Level::Low);
        assert_points(
            &pwl(sequence.voltage_waveform(&output)),
            &[
                (0.0, 0.0),
                (0.2, 2.0),
                (2.0, 2.0),
                (2.4, 0.0),
                (3.0, 0.0),
                (3.1, 1.0),
                (3.3, 0.0),
            ],
        );
        assert_points(
            &pwl(sequence.enable_waveform(0.01).unwrap()),
            &[(0.0, 1.0), (1.0, 1.0), (1.01, 0.0), (2.0, 0.0), (2.01, 1.0)],
        );
        assert_eq!(
            DigitalSequence::new(Level::Low)
                .at(1.0, Level::High)
                .enable_waveform(0.01),
            None
        );
        // Starting in HiZ: the source waits at `low`.
        let open_drain = DigitalSequence::new(Level::HiZ).at(1.0, Level::Low);
        assert_points(&pwl(open_drain.voltage_waveform(&output)), &[(0.0, 0.0)]);
    }

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
                 VD d ref PWL( 0 0 1e-6 0 {edge_end} 3.3 )\n\
                 RO d out 50\n\
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
                "VE e ref PWL( 0 1 1e-6 1 {edge_end} 0 )\n\
                     SO d out e ref hiz\n\
                     .model hiz SW( vt=0.5 vh=0 ron=50 roff=1e9 )\n"
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
