//! Simulation-only parts for test benches: independent sources and an ammeter.
//!
//! They have no KiCad information, so a circuit containing them cannot be exported to KiCad.
//! Put them in a test bench circuit that instantiates the circuit under test as a subcircuit.

use crate::{
    parts::Part,
    spice::{SpiceElement, SpicePartInfo},
};

/// Formats a number for SPICE: plain decimal for moderate magnitudes, exponent form otherwise.
///
/// # Panics
/// If `value` is NaN or infinite, which ngspice cannot read.
#[track_caller]
pub fn spice_number(value: f64) -> String {
    assert!(value.is_finite(), "a SPICE number must be finite: {value}");
    let magnitude = value.abs();
    if value == 0.0 || (1e-3..1e6).contains(&magnitude) {
        format!("{value}")
    } else {
        format!("{value:e}")
    }
}

/// Whether an [`IndependentSource`] is a voltage or a current source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A voltage source (SPICE `V`); its values are in volts.
    Voltage,
    /// A current source (SPICE `I`); its values are in amperes.
    Current,
}

/// A time-dependent waveform for transient analysis.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Waveform {
    /// `SIN(offset amplitude frequency delay damping phase)`; phase in degrees.
    Sin {
        /// The value the wave oscillates around.
        offset: f64,
        /// The peak deviation from `offset`.
        amplitude: f64,
        /// The frequency in hertz.
        frequency: f64,
        /// The time in seconds at which the wave starts.
        delay: f64,
        /// The damping factor in 1/s: the amplitude decays as `exp(-damping * (t - delay))`.
        damping: f64,
        /// The phase in degrees.
        phase: f64,
    },
    /// `PULSE(initial pulsed delay rise fall width period)`.
    Pulse {
        /// The value before `delay`.
        initial: f64,
        /// The value at the top of the pulse.
        pulsed: f64,
        /// The time in seconds of the first edge.
        delay: f64,
        /// The rise time in seconds (from `initial` to `pulsed`).
        rise: f64,
        /// The fall time in seconds (from `pulsed` back to `initial`).
        fall: f64,
        /// The time in seconds the value stays at `pulsed`.
        width: f64,
        /// The repetition period in seconds.
        period: f64,
    },
    /// Piecewise linear: `(time, value)` pairs in strictly increasing time (at least one).
    Pwl(Vec<(f64, f64)>),
}

impl Waveform {
    /// A sine wave without delay, damping or phase.
    pub fn sin(offset: f64, amplitude: f64, frequency: f64) -> Self {
        Waveform::Sin {
            offset,
            amplitude,
            frequency,
            delay: 0.0,
            damping: 0.0,
            phase: 0.0,
        }
    }

    /// The keyword of the waveform in SPICE.
    fn keyword(&self) -> &'static str {
        match self {
            Waveform::Sin { .. } => "SIN",
            Waveform::Pulse { .. } => "PULSE",
            Waveform::Pwl(_) => "PWL",
        }
    }

    /// The numbers of the waveform, in the order SPICE takes them.
    fn numbers(&self) -> Vec<f64> {
        match self {
            Waveform::Sin {
                offset,
                amplitude,
                frequency,
                delay,
                damping,
                phase,
            } => vec![*offset, *amplitude, *frequency, *delay, *damping, *phase],
            Waveform::Pulse {
                initial,
                pulsed,
                delay,
                rise,
                fall,
                width,
                period,
            } => vec![*initial, *pulsed, *delay, *rise, *fall, *width, *period],
            Waveform::Pwl(points) => points.iter().flat_map(|&(t, v)| [t, v]).collect(),
        }
    }

    /// # Panics
    /// If a number is not finite, or the points of `Pwl` are none or not in increasing time.
    #[track_caller]
    fn validate(&self) {
        if let Waveform::Pwl(points) = self {
            assert!(!points.is_empty(), "a PWL waveform needs a point");
            assert!(
                points.windows(2).all(|pair| pair[0].0 < pair[1].0),
                "the times of a PWL waveform must increase: {points:?}"
            );
        }
        assert!(
            self.numbers().iter().all(|v| v.is_finite()),
            "the numbers of a waveform must be finite: {self:?}"
        );
    }

    pub(crate) fn to_spice(&self) -> String {
        let numbers: Vec<String> = self.numbers().into_iter().map(spice_number).collect();
        format!("{}( {} )", self.keyword(), numbers.join(" "))
    }
}

/// An independent voltage or current source with pins `p` and `n`.
///
/// A voltage source drives `p` to `dc` volts above `n`. A current source drives `dc` amperes
/// from `p` through the source to `n` (so into the circuit at `n`), as in SPICE.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct IndependentSource {
    /// Whether the source drives a voltage or a current.
    pub kind: SourceKind,
    /// The DC value (volts or amperes, by `kind`), used by the operating point and DC analyses
    /// and by a transient analysis without `waveform`.
    pub dc: f64,
    /// Magnitude for AC (small-signal) analysis.
    pub ac: Option<f64>,
    /// The waveform for transient analysis, in the unit of `kind`.
    pub waveform: Option<Waveform>,
}

impl IndependentSource {
    /// The positive pin.
    pub const P: &'static str = "p";
    /// The negative pin.
    pub const N: &'static str = "n";

    /// # Panics
    /// If `dc` is not finite.
    #[track_caller]
    pub fn voltage(dc: f64) -> Self {
        let source = IndependentSource {
            kind: SourceKind::Voltage,
            dc,
            ac: None,
            waveform: None,
        };
        source.validate();
        source
    }

    /// # Panics
    /// If `dc` is not finite.
    #[track_caller]
    pub fn current(dc: f64) -> Self {
        IndependentSource {
            kind: SourceKind::Current,
            ..IndependentSource::voltage(dc)
        }
    }

    /// # Panics
    /// If `magnitude` is not finite.
    #[track_caller]
    pub fn with_ac(mut self, magnitude: f64) -> Self {
        self.ac = Some(magnitude);
        self.validate();
        self
    }

    /// # Panics
    /// As [`Waveform`]'s numbers are not finite, or the points of `Pwl` are none or not in
    /// increasing time.
    #[track_caller]
    pub fn with_waveform(mut self, waveform: Waveform) -> Self {
        self.waveform = Some(waveform);
        self.validate();
        self
    }

    /// Checks the values, also when the netlist is written (the fields are public).
    #[track_caller]
    fn validate(&self) {
        assert!(
            self.dc.is_finite() && self.ac.is_none_or(f64::is_finite),
            "the values of a source must be finite: DC {}, AC {:?}",
            self.dc,
            self.ac
        );
        if let Some(waveform) = &self.waveform {
            waveform.validate();
        }
    }

    fn device(&self) -> char {
        match self.kind {
            SourceKind::Voltage => 'V',
            SourceKind::Current => 'I',
        }
    }

    fn args(&self) -> String {
        self.validate();
        let mut args = format!("DC {}", spice_number(self.dc));
        if let Some(ac) = self.ac {
            args.push_str(&format!(" AC {}", spice_number(ac)));
        }
        if let Some(waveform) = &self.waveform {
            args.push(' ');
            args.push_str(&waveform.to_spice());
        }
        args
    }
}

impl Part for IndependentSource {
    fn ref_prefix(&self) -> String {
        self.device().to_string()
    }
    fn pin_ids(&self) -> Vec<String> {
        vec![Self::P.into(), Self::N.into()]
    }
    fn value(&self) -> String {
        self.args()
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        Some(SpicePartInfo::element(SpiceElement::new(
            self.device(),
            [Self::P, Self::N],
            self.args(),
        )))
    }
}

/// A 0 V source for measuring a current, with pins `p` and `n`. The current flowing into `p`
/// and out of `n` is the vector `i(VM<n>)` (references are `VM1`, `VM2`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ammeter;

impl Ammeter {
    /// The pin the measured current flows into.
    pub const P: &'static str = "p";
    /// The pin the measured current flows out of.
    pub const N: &'static str = "n";
}

impl Part for Ammeter {
    fn ref_prefix(&self) -> String {
        "VM".into()
    }
    fn pin_ids(&self) -> Vec<String> {
        vec![Self::P.into(), Self::N.into()]
    }
    fn value(&self) -> String {
        "DC 0".into()
    }
    fn spice(&self) -> Option<SpicePartInfo> {
        Some(SpicePartInfo::element(SpiceElement::new(
            'V',
            [Self::P, Self::N],
            "DC 0",
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spice_number() {
        assert_eq!(spice_number(0.0), "0");
        assert_eq!(spice_number(5.0), "5");
        assert_eq!(spice_number(-1.5), "-1.5");
        assert_eq!(spice_number(0.001), "0.001");
        assert_eq!(spice_number(1e-9), "1e-9");
        assert_eq!(spice_number(2.5e6), "2.5e6");
    }

    #[test]
    #[should_panic(expected = "a SPICE number must be finite: NaN")]
    fn test_spice_number_nan() {
        spice_number(f64::NAN);
    }

    #[test]
    #[should_panic(expected = "the values of a source must be finite")]
    fn test_source_infinite() {
        IndependentSource::voltage(0.0).with_ac(f64::INFINITY);
    }

    #[test]
    #[should_panic(expected = "the times of a PWL waveform must increase")]
    fn test_pwl_not_increasing() {
        IndependentSource::voltage(0.0)
            .with_waveform(Waveform::Pwl(vec![(1e-6, 0.0), (1e-6, 1.0)]));
    }

    #[test]
    #[should_panic(expected = "the numbers of a waveform must be finite")]
    fn test_waveform_nan() {
        IndependentSource::voltage(0.0).with_waveform(Waveform::sin(0.0, 1.0, f64::NAN));
    }

    /// The fields are public: they are checked again when the netlist is written.
    #[test]
    #[should_panic(expected = "the values of a source must be finite")]
    fn test_source_field_changed() {
        let mut source = IndependentSource::voltage(0.0);
        source.dc = f64::NAN;
        source.spice();
    }

    #[test]
    fn test_source_args() {
        let source = IndependentSource::voltage(0.0)
            .with_ac(1.0)
            .with_waveform(Waveform::sin(0.0, 1.0, 1e3));
        assert_eq!(source.value(), "DC 0 AC 1 SIN( 0 1 1000 0 0 0 )");
        let pulse = Waveform::Pulse {
            initial: 0.0,
            pulsed: 3.3,
            delay: 1e-6,
            rise: 1e-9,
            fall: 1e-9,
            width: 5e-6,
            period: 10e-6,
        };
        assert_eq!(
            IndependentSource::current(0.0).with_waveform(pulse).value(),
            "DC 0 PULSE( 0 3.3 1e-6 1e-9 1e-9 5e-6 1e-5 )"
        );
        let pwl = Waveform::Pwl(vec![(0.0, 0.0), (1e-3, 1.0)]);
        assert_eq!(
            IndependentSource::voltage(0.0).with_waveform(pwl).value(),
            "DC 0 PWL( 0 0 0.001 1 )"
        );
        assert_eq!(IndependentSource::current(1e-3).ref_prefix(), "I");
    }
}
