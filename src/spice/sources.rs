//! Simulation-only parts for test benches: independent sources and an ammeter.
//!
//! They have no KiCad information, so a circuit containing them cannot be exported to KiCad.
//! Put them in a test bench circuit that instantiates the circuit under test as a subcircuit.

use crate::{
    parts::Part,
    spice::{SpiceElement, SpicePartInfo},
};

/// Formats a number for SPICE: plain decimal for moderate magnitudes, exponent form otherwise.
pub fn spice_number(value: f64) -> String {
    let magnitude = value.abs();
    if value == 0.0 || (1e-3..1e6).contains(&magnitude) {
        format!("{value}")
    } else {
        format!("{value:e}")
    }
}

fn numbers(values: &[f64]) -> String {
    values
        .iter()
        .map(|v| spice_number(*v))
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Voltage,
    Current,
}

/// A time-dependent waveform for transient analysis.
#[derive(Debug, Clone, PartialEq)]
pub enum Waveform {
    /// `SIN(offset amplitude frequency delay damping phase)`; phase in degrees.
    Sin {
        offset: f64,
        amplitude: f64,
        frequency: f64,
        delay: f64,
        damping: f64,
        phase: f64,
    },
    /// `PULSE(initial pulsed delay rise fall width period)`.
    Pulse {
        initial: f64,
        pulsed: f64,
        delay: f64,
        rise: f64,
        fall: f64,
        width: f64,
        period: f64,
    },
    /// Piecewise linear: `(time, value)` pairs in increasing time.
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

    pub(crate) fn to_spice(&self) -> String {
        match self {
            Waveform::Sin {
                offset,
                amplitude,
                frequency,
                delay,
                damping,
                phase,
            } => format!(
                "SIN( {} )",
                numbers(&[*offset, *amplitude, *frequency, *delay, *damping, *phase])
            ),
            Waveform::Pulse {
                initial,
                pulsed,
                delay,
                rise,
                fall,
                width,
                period,
            } => format!(
                "PULSE( {} )",
                numbers(&[*initial, *pulsed, *delay, *rise, *fall, *width, *period])
            ),
            Waveform::Pwl(points) => {
                let values: Vec<f64> = points.iter().flat_map(|&(t, v)| [t, v]).collect();
                format!("PWL( {} )", numbers(&values))
            }
        }
    }
}

/// An independent voltage or current source with pins `p` and `n`.
///
/// A voltage source drives `p` to `dc` volts above `n`. A current source drives `dc` amperes
/// from `p` through the source to `n` (so into the circuit at `n`), as in SPICE.
#[derive(Debug, Clone, PartialEq)]
pub struct IndependentSource {
    pub kind: SourceKind,
    pub dc: f64,
    /// Magnitude for AC (small-signal) analysis.
    pub ac: Option<f64>,
    pub waveform: Option<Waveform>,
}

impl IndependentSource {
    /// The positive pin.
    pub const P: &'static str = "p";
    /// The negative pin.
    pub const N: &'static str = "n";

    pub fn voltage(dc: f64) -> Box<Self> {
        Box::new(IndependentSource {
            kind: SourceKind::Voltage,
            dc,
            ac: None,
            waveform: None,
        })
    }

    pub fn current(dc: f64) -> Box<Self> {
        Box::new(IndependentSource {
            kind: SourceKind::Current,
            ..*IndependentSource::voltage(dc)
        })
    }

    pub fn with_ac(mut self: Box<Self>, magnitude: f64) -> Box<Self> {
        self.ac = Some(magnitude);
        self
    }

    pub fn with_waveform(mut self: Box<Self>, waveform: Waveform) -> Box<Self> {
        self.waveform = Some(waveform);
        self
    }

    fn device(&self) -> char {
        match self.kind {
            SourceKind::Voltage => 'V',
            SourceKind::Current => 'I',
        }
    }

    fn args(&self) -> String {
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
    fn get_ref_base_name(&self) -> String {
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
    fn get_ref_base_name(&self) -> String {
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
        assert_eq!(IndependentSource::current(1e-3).get_ref_base_name(), "I");
    }
}
