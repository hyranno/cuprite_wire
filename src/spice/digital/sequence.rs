//! Sequences of logic levels and the waveforms they make.

use super::{Level, OutputCharacteristics};
use crate::spice::sources::Waveform;

/// A sequence of logic levels in time: an initial level and the times at which it changes.
#[derive(Debug, Clone, PartialEq)]
pub struct DigitalSequence {
    initial: Level,
    events: Vec<(f64, Level)>,
}

impl DigitalSequence {
    /// A sequence that stays at `initial` from time 0, without changes.
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
    ///
    /// # Panics
    /// If `time` is negative, not finite, or before the last change.
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

    /// The level at time 0.
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
    ///
    /// # Panics
    /// If `baud` is not positive and finite, or `start` is negative, not finite, or before the
    /// last change.
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
    ///
    /// # Panics
    /// If `period` is not positive and finite, `duty` is not in `0..=1`, or `start` is negative,
    /// not finite, or before the last change.
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
    /// If `output` is invalid (see [`DigitalDriver::new`](super::DigitalDriver::new)).
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
        if current == target {
            // The cut ramp had not left `target` yet (two events at one time).
            return;
        }
        self.points.push((time + duration(current, target), target));
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
    fn test_waveform_events_at_one_time() {
        // `Low` cuts the edge to `High` at its start: the output stays low, with no repeated time.
        let sequence = DigitalSequence::new(Level::Low)
            .at(1.0, Level::High)
            .at(1.0, Level::Low);
        let output = OutputCharacteristics::cmos(1.0);
        let Waveform::Pwl(points) = sequence.voltage_waveform(&output) else {
            unreachable!()
        };
        assert_points(&points, &[(0.0, 0.0), (1.0, 0.0)]);
        let enable = DigitalSequence::new(Level::HiZ)
            .at(1.0, Level::High)
            .at(1.0, Level::HiZ)
            .enable_waveform(1e-9)
            .unwrap();
        let Waveform::Pwl(points) = &enable else {
            unreachable!()
        };
        assert_points(points, &[(0.0, 0.0), (1.0, 0.0)]);
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
}
