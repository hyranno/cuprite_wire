//! Models of common peripherals of microcontrollers, to build a
//! [`DeviceModel`](super::DeviceModel) with. Each is driven from [`DeviceModel::step`] with the
//! [`DeviceIo`] of the device.
//!
//! [`DeviceModel::step`]: super::DeviceModel::step

use super::DeviceIo;
use crate::spice::digital::Level;

/// Whether `time` has reached `target`, allowing for the rounding of breakpoints by ngspice.
fn reached(time: f64, target: f64) -> bool {
    time >= target - (target.abs() * 1e-9 + 1e-15)
}

/// A task run every `period` from `start` (a timer interrupt, the loop of a control algorithm).
///
/// ```
/// # use cuprite_wire::spice::cosim::{DeviceIo, DeviceModel, peripherals::Periodic};
/// struct Blink { tick: Periodic, on: bool }
///
/// impl DeviceModel for Blink {
///     fn step(&mut self, io: &mut DeviceIo<'_>) {
///         if self.tick.due(io) {
///             self.on = !self.on;
///             io.set("led", self.on.into());
///         }
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Periodic {
    period: f64,
    next: f64,
    scheduled: bool,
}

impl Periodic {
    /// # Panics
    /// If `period` is not positive or `start` is negative.
    #[track_caller]
    pub fn new(period: f64, start: f64) -> Self {
        assert!(
            period.is_finite() && period > 0.0,
            "the period must be positive: {period}"
        );
        assert!(
            start.is_finite() && start >= 0.0,
            "the start must not be negative: {start}"
        );
        Periodic {
            period,
            next: start,
            scheduled: false,
        }
    }

    pub fn period(&self) -> f64 {
        self.period
    }

    /// Whether the task is due at the current time; call it at every step. Asks for a time point
    /// at the next run. Runs missed (when the time points are further apart than the period) are
    /// dropped.
    pub fn due(&mut self, io: &mut DeviceIo<'_>) -> bool {
        let time = io.time();
        let due = reached(time, self.next);
        if due {
            while reached(time, self.next) {
                self.next += self.period;
            }
            self.scheduled = false;
        }
        if !self.scheduled {
            io.wake_at(self.next);
            self.scheduled = true;
        }
        due
    }
}

/// An ADC converting the voltage of a pin to `bits` bits, full scale at `reference` volts
/// (`code = floor(voltage / reference * 2^bits)`, clamped). It samples when it is read.
#[derive(Debug, Clone, PartialEq)]
pub struct Adc {
    pub pin: String,
    pub bits: u32,
    pub reference: f64,
}

impl Adc {
    /// # Panics
    /// If `bits` is not in 1..=31 or `reference` is not positive.
    #[track_caller]
    pub fn new(pin: impl Into<String>, bits: u32, reference: f64) -> Self {
        assert!(
            (1..=31).contains(&bits),
            "the resolution must be 1 to 31 bits: {bits}"
        );
        assert!(
            reference.is_finite() && reference > 0.0,
            "the reference voltage must be positive: {reference}"
        );
        Adc {
            pin: pin.into(),
            bits,
            reference,
        }
    }

    /// The largest code.
    pub fn max_code(&self) -> u32 {
        (1 << self.bits) - 1
    }

    /// Converts the voltage of the pin now.
    #[track_caller]
    pub fn read(&self, io: &DeviceIo<'_>) -> u32 {
        let scaled = io.voltage(&self.pin) / self.reference * f64::from(1u32 << self.bits);
        scaled.floor().clamp(0.0, f64::from(self.max_code())) as u32
    }

    /// The voltage a code stands for (the bottom of its step).
    pub fn voltage_of(&self, code: u32) -> f64 {
        f64::from(code) * self.reference / f64::from(1u32 << self.bits)
    }
}

/// A PWM output: `High` for `duty * period` at the start of each period, then `Low`.
/// A new duty takes effect at the next period, as with the buffered compare registers of timers.
#[derive(Debug, Clone, PartialEq)]
pub struct Pwm {
    pin: String,
    timer: Periodic,
    duty: f64,
    /// The end of the high time of the current period, until it is reached.
    fall: Option<f64>,
}

impl Pwm {
    /// Starts at `start` with a duty of 0.
    ///
    /// # Panics
    /// As [`Periodic::new`].
    #[track_caller]
    pub fn new(pin: impl Into<String>, period: f64, start: f64) -> Self {
        Pwm {
            pin: pin.into(),
            timer: Periodic::new(period, start),
            duty: 0.0,
            fall: None,
        }
    }

    pub fn duty(&self) -> f64 {
        self.duty
    }

    /// Sets the duty for the next periods, clamped to 0..=1.
    pub fn set_duty(&mut self, duty: f64) {
        self.duty = duty.clamp(0.0, 1.0);
    }

    /// Drives the pin; call it at every step.
    #[track_caller]
    pub fn update(&mut self, io: &mut DeviceIo<'_>) {
        let time = io.time();
        if let Some(fall) = self.fall
            && reached(time, fall)
        {
            io.set(&self.pin, Level::Low);
            self.fall = None;
        }
        if self.timer.due(io) {
            let high = self.duty * self.timer.period();
            if high <= 0.0 {
                io.set(&self.pin, Level::Low);
                self.fall = None;
            } else {
                io.set(&self.pin, Level::High);
                self.fall = (self.duty < 1.0).then_some(time + high);
                if let Some(fall) = self.fall {
                    io.wake_at(fall);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spice::{
        cosim::{CosimPin, PinState},
        digital::{InputCharacteristics, OutputCharacteristics},
    };

    fn pins() -> Vec<PinState> {
        [("adc", false), ("pwm", true)]
            .into_iter()
            .map(|(name, output)| PinState {
                pin: CosimPin {
                    name: name.into(),
                    output: output.then(|| OutputCharacteristics::cmos(3.3)),
                    input: (!output).then(|| InputCharacteristics::cmos(3.3)),
                },
                voltage: 0.0,
                read: false,
                request: None,
            })
            .collect()
    }

    #[test]
    fn test_adc() {
        let adc = Adc::new("adc", 10, 2.048);
        let mut pins = pins();
        let mut wakes = Vec::new();
        for (voltage, code) in [
            (-0.1, 0),
            (1.0, 500),
            (1.0019, 500),
            (1.002, 501),
            (3.0, 1023),
        ] {
            pins[0].voltage = voltage;
            let io = DeviceIo {
                time: 0.0,
                pins: &mut pins,
                wakes: &mut wakes,
            };
            assert_eq!(adc.read(&io), code, "{voltage} V");
        }
        assert_eq!(adc.voltage_of(500), 1.0);
    }

    #[test]
    fn test_periodic_and_pwm() {
        let mut pins = pins();
        let mut wakes = Vec::new();
        let mut timer = Periodic::new(1.0, 0.5);
        let mut pwm = Pwm::new("pwm", 1.0, 0.0);
        pwm.set_duty(0.25);
        let mut run = |time: f64| {
            wakes.clear();
            let mut io = DeviceIo {
                time,
                pins: &mut pins,
                wakes: &mut wakes,
            };
            let due = timer.due(&mut io);
            pwm.update(&mut io);
            (due, pins[1].request.take(), wakes.clone())
        };
        use super::super::Drive::Level as L;
        assert_eq!(
            run(0.0),
            (false, Some(L(Level::High)), vec![0.5, 1.0, 0.25])
        );
        assert_eq!(run(0.25), (false, Some(L(Level::Low)), vec![]));
        assert_eq!(run(0.5), (true, None, vec![1.5]));
        // Missed runs are dropped.
        assert_eq!(run(2.7), (true, Some(L(Level::High)), vec![3.5, 3.0, 2.95]));
    }
}
