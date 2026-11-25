//! The edges of a driver: how its weights `ku` and `kd` change as it switches.

use super::IvCurve;
use crate::{Corner, Ramp, Waveform};

/// What a driver does to its pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Drive {
    /// Driven low: `[Pulldown]` on (`kd` = 1, `ku` = 0).
    Low,
    /// Driven high: `[Pullup]` on (`ku` = 1, `kd` = 0).
    High,
    /// High impedance: both off. An open drain output is `HiZ` or `Low`.
    HiZ,
}

impl Drive {
    /// The weights (`ku`, `kd`).
    fn weights(self) -> (f64, f64) {
        match self {
            Drive::Low => (0.0, 1.0),
            Drive::High => (1.0, 0.0),
            Drive::HiZ => (0.0, 0.0),
        }
    }
}

/// How the weights change over an edge from low to high or back, as (time, weight) points in
/// increasing time from the start of the edge (time 0).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Edge {
    /// The weight of `[Pullup]`: from 0 to 1 on a rising edge, from 1 to 0 on a falling one.
    pub ku: Vec<(f64, f64)>,
    /// The weight of `[Pulldown]`: from 1 to 0 on a rising edge, from 0 to 1 on a falling one.
    pub kd: Vec<(f64, f64)>,
}

impl Edge {
    /// A linear edge over `duration`.
    fn linear(duration: f64, rising: bool) -> Self {
        let (from, to) = if rising { (0.0, 1.0) } else { (1.0, 0.0) };
        Edge {
            ku: vec![(0.0, from), (duration, to)],
            kd: vec![(0.0, to), (duration, from)],
        }
    }

    /// The time until both weights reach their ends, in seconds.
    pub fn duration(&self) -> f64 {
        let end = |points: &[(f64, f64)]| points.last().map_or(0.0, |p| p.0);
        end(&self.ku).max(end(&self.kd))
    }
}

/// The rising and falling edges of a driver.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Edges {
    /// From low to high.
    pub rising: Edge,
    /// From high to low.
    pub falling: Edge,
}

impl Edges {
    /// Linear edges, the rising one over `rise` and the falling one over `fall` (in seconds).
    ///
    /// # Panics
    /// If a time is not positive.
    #[track_caller]
    pub fn linear(rise: f64, fall: f64) -> Self {
        assert!(
            rise > 0.0 && fall > 0.0,
            "the edge times must be positive: rise {rise}, fall {fall}"
        );
        Edges {
            rising: Edge::linear(rise, true),
            falling: Edge::linear(fall, false),
        }
    }

    /// The linear edges of the corner of a `[Ramp]`, as KIBIS takes them: the `dt` of `dV/dt`,
    /// the time from 20% to 80% of the swing, extended to the whole swing (`dt / 0.6`).
    ///
    /// # Panics
    /// If a `dt` is not positive.
    #[track_caller]
    pub fn from_ramp(ramp: &Ramp, corner: Corner) -> Self {
        Self::linear(
            ramp_time(ramp, corner, true),
            ramp_time(ramp, corner, false),
        )
    }

    /// The piecewise linear waveforms of `ku` and `kd` that start in the state `initial` and
    /// change to the state of each event at its time (in seconds, non-decreasing and not
    /// negative).
    ///
    /// Each weight follows the curve of the edge that moves it the same way: `ku` rises as in
    /// [`rising`](Self::rising) and falls as in [`falling`](Self::falling), `kd` rises as in
    /// `falling` and falls as in `rising`. So a change to `HiZ` from `High` follows the falling
    /// `ku`, and from `Low` the rising `kd`. A change that starts before the previous one ends
    /// starts from where that one has got to: from the point of the curve that has come as far.
    ///
    /// # Panics
    /// If the event times are negative or not in order.
    #[track_caller]
    pub fn weights(&self, initial: Drive, events: &[(f64, Drive)]) -> Weights {
        let (ku, kd) = initial.weights();
        let mut weights = Weights {
            ku: vec![(0.0, ku)],
            kd: vec![(0.0, kd)],
        };
        let mut state = initial;
        let mut last_time = 0.0;
        for &(time, next) in events {
            assert!(
                time >= last_time,
                "the event times must be non-negative and in order: {time} after {last_time}"
            );
            last_time = time;
            if next == state {
                continue;
            }
            let (ku, kd) = next.weights();
            let ku_curve = if ku > 0.0 {
                &self.rising
            } else {
                &self.falling
            };
            let kd_curve = if kd > 0.0 {
                &self.falling
            } else {
                &self.rising
            };
            follow(&mut weights.ku, time, ku, &ku_curve.ku);
            follow(&mut weights.kd, time, kd, &kd_curve.kd);
            state = next;
        }
        weights
    }
}

fn ramp_time(ramp: &Ramp, corner: Corner, rising: bool) -> f64 {
    let slope = if rising { ramp.dv_dt_r } else { ramp.dv_dt_f };
    slope.get(corner).dt / 0.6
}

/// The waveforms of the ports `ku` and `kd`, as (time, value) points in increasing time.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Weights {
    /// The weight of `[Pullup]`.
    pub ku: Vec<(f64, f64)>,
    /// The weight of `[Pulldown]`.
    pub kd: Vec<(f64, f64)>,
}

/// Changes a waveform to `target` from `time` along `curve`, which ends at `target`.
fn follow(points: &mut Vec<(f64, f64)>, time: f64, target: f64, curve: &[(f64, f64)]) {
    let value = value_at(points, time);
    // The points after `time` are the rest of a change in progress.
    points.retain(|&(t, _)| t <= time);
    if value == target {
        // Hold the value reached (the end of a change cut short, or the last point).
        if points.last().is_some_and(|&(_, v)| v != value) {
            points.push((time, value));
        }
        return;
    }
    if points.last().is_some_and(|&(t, _)| t < time) {
        points.push((time, value));
    }
    // Start at the first point of the curve that has come as far as `value`.
    let start = curve[0].1;
    let progress = |v: f64| (v - start) / (target - start);
    let reached = progress(value);
    let offset = match curve.iter().position(|&(_, v)| progress(v) >= reached) {
        Some(0) | None => 0.0,
        Some(i) => {
            let ((t0, v0), (t1, v1)) = (curve[i - 1], curve[i]);
            let (p0, p1) = (progress(v0), progress(v1));
            t0 + (t1 - t0) * (reached - p0) / (p1 - p0)
        }
    };
    points.extend(
        curve
            .iter()
            .filter(|&&(t, _)| t > offset)
            .map(|&(t, v)| (time + t - offset, v)),
    );
}

/// The value of a piecewise linear waveform at `time` (the last value after the end).
fn value_at(points: &[(f64, f64)], time: f64) -> f64 {
    let after = points.partition_point(|&(t, _)| t <= time);
    match (after.checked_sub(1).map(|i| points[i]), points.get(after)) {
        (Some((t0, v0)), Some(&(t1, v1))) => v0 + (v1 - v0) * (time - t0) / (t1 - t0),
        (Some((_, v)), None) => v,
        (None, _) => points[0].1,
    }
}

/// The largest error, in weight, of the piecewise linear curves made from waveforms.
const TOLERANCE: f64 = 1e-3;

/// The model of a driver, as the waveforms need it.
pub(super) struct Buffer<'a> {
    pub pullup: Option<&'a IvCurve>,
    pub pulldown: Option<&'a IvCurve>,
    pub clamps: Vec<&'a IvCurve>,
    pub c_comp: f64,
}

/// The edge of a direction from the waveforms of the model, or `None` without usable ones. The
/// waveforms left out are explained in `notes`.
pub(super) fn from_waveforms(
    buffer: &Buffer,
    waveforms: &[Waveform],
    rising: bool,
    corner: Corner,
    notes: &mut Vec<String>,
) -> Option<Edge> {
    let keyword = if rising {
        "[Rising Waveform]"
    } else {
        "[Falling Waveform]"
    };
    let usable: Vec<&Waveform> = waveforms
        .iter()
        .enumerate()
        .filter(|(i, w)| {
            let unsupported: Vec<_> = [
                ("L_fixture", w.l_fixture),
                ("R_dut", w.r_dut),
                ("L_dut", w.l_dut),
                ("C_dut", w.c_dut),
            ]
            .into_iter()
            .filter_map(|(name, value)| value.filter(|&v| v != 0.0).map(|v| format!("{name} {v}")))
            .collect();
            if !unsupported.is_empty() {
                notes.push(format!(
                    "{keyword} {} is not used: {} not supported",
                    i + 1,
                    unsupported.join(", ")
                ));
            }
            unsupported.is_empty()
        })
        .map(|(_, w)| w)
        .collect();
    // Two waveforms with fixtures as far apart as can be, when both weights are unknown.
    let mut chosen = usable.first().map(|&w| vec![w])?;
    if buffer.pullup.is_some() && buffer.pulldown.is_some() {
        let pairs = usable
            .iter()
            .enumerate()
            .flat_map(|(i, a)| usable[i + 1..].iter().map(move |b| (*a, *b)));
        let spread = |(a, b): (&Waveform, &Waveform)| {
            (a.v_fixture.get(corner) - b.v_fixture.get(corner)).abs()
        };
        if let Some(pair) = pairs.max_by(|&x, &y| spread(x).total_cmp(&spread(y)))
            && spread(pair) > 1e-3
        {
            chosen = vec![pair.0, pair.1];
        }
    }
    Some(solve(buffer, &chosen, rising, corner))
}

/// How small a determinant may be relative to its terms before the equations are taken as
/// dependent: the solution of nearly dependent equations is mostly rounding error, and clamping it
/// to 0..=1 makes the weight jump between 0 and 1.
const SINGULAR: f64 = 1e-9;

/// Solves `a·ku + c·kd = b` for two rows `(a, c, b)`; `None` if they are (nearly) dependent.
fn solve_pair((a1, c1, b1): (f64, f64, f64), (a2, c2, b2): (f64, f64, f64)) -> Option<(f64, f64)> {
    let det = a1 * c2 - a2 * c1;
    if det.abs() <= SINGULAR * ((a1 * c2).abs() + (a2 * c1).abs()) {
        return None;
    }
    let solution = ((b1 * c2 - b2 * c1) / det, (a1 * b2 - a2 * b1) / det);
    (solution.0.is_finite() && solution.1.is_finite()).then_some(solution)
}

/// `b / a`; `None` if `a` is 0 or the quotient is not finite.
fn divide(b: f64, a: f64) -> Option<f64> {
    Some(b / a).filter(|q| a != 0.0 && q.is_finite())
}

/// Solves the weights at each time of the waveforms, so that the buffer drives the current that
/// flows into each fixture. Where the equations are (nearly) dependent, the weights keep their
/// previous values.
fn solve(buffer: &Buffer, waveforms: &[&Waveform], rising: bool, corner: Corner) -> Edge {
    let curves: Vec<Vec<(f64, f64)>> = waveforms.iter().map(|w| w.curve(corner)).collect();
    let end = curves
        .iter()
        .map(|c| c.last().unwrap().0)
        .fold(f64::INFINITY, f64::min);
    // From the start of the edge, time 0: a table may start before or after it.
    let mut times: Vec<f64> = curves
        .iter()
        .flatten()
        .map(|p| p.0)
        .filter(|&t| (0.0..=end).contains(&t))
        .chain([0.0])
        .collect();
    times.sort_by(f64::total_cmp);
    times.dedup();

    // The equation of each waveform at each time: a·ku + c·kd = b.
    let rows = |k: usize, i: usize| {
        let w = waveforms[k];
        let v = |i: usize| value_at(&curves[k], times[i]);
        let (before, after) = (i.saturating_sub(1), (i + 1).min(times.len() - 1));
        let dv_dt = (v(after) - v(before)) / (times[after] - times[before]);
        let voltage = v(i);
        let fixture =
            (w.v_fixture.get(corner) - voltage) / w.r_fixture - w.c_fixture.unwrap_or(0.0) * dv_dt;
        let clamps: f64 = buffer.clamps.iter().map(|c| c.current(voltage)).sum();
        let b = fixture - clamps - buffer.c_comp * dv_dt;
        let a = buffer.pullup.map_or(0.0, |c| c.current(voltage));
        let c = buffer.pulldown.map_or(0.0, |c| c.current(voltage));
        (a, c, b)
    };
    let (ku_from, ku_to) = if rising { (0.0, 1.0) } else { (1.0, 0.0) };
    let (has_pu, has_pd) = (buffer.pullup.is_some(), buffer.pulldown.is_some());
    let mut previous = (ku_from, 1.0 - ku_from);
    let mut ku = Vec::with_capacity(times.len());
    let mut kd = Vec::with_capacity(times.len());
    for (i, &t) in times.iter().enumerate() {
        let first = rows(0, i);
        let solution = match (has_pu, has_pd, waveforms.len()) {
            (true, true, 2) => solve_pair(first, rows(1, i)),
            // One waveform: ku + kd = 1, that is (a - c)·ku = b - c.
            (true, true, _) => {
                let (a, c, b) = first;
                solve_pair((a, c, b), (1.0, 1.0, 1.0))
            }
            (true, false, _) => divide(first.2, first.0).map(|ku| (ku, 0.0)),
            _ => divide(first.2, first.1).map(|kd| (0.0, kd)),
        };
        if let Some((ku, kd)) = solution {
            // `+ 0.0` turns -0 into 0.
            previous = (ku.clamp(0.0, 1.0) + 0.0, kd.clamp(0.0, 1.0) + 0.0);
        }
        ku.push((t, previous.0));
        kd.push((t, previous.1));
    }
    // The edge starts and ends at the levels, whatever the tables and the waveforms miss.
    let pin = |points: &mut Vec<(f64, f64)>, from: f64, to: f64| {
        points[0].1 = from;
        points.last_mut().unwrap().1 = to;
        *points = simplify(points, TOLERANCE);
    };
    pin(&mut ku, ku_from, ku_to);
    pin(&mut kd, ku_to, ku_from);
    // A weight without its table mirrors the other, so that it still goes from end to end.
    let mirror = |points: &[(f64, f64)]| points.iter().map(|&(t, v)| (t, 1.0 - v)).collect();
    if !has_pu {
        ku = mirror(&kd);
    } else if !has_pd {
        kd = mirror(&ku);
    }
    Edge { ku, kd }
}

/// The points of a piecewise linear curve that keep it within `tolerance` (Ramer-Douglas-Peucker).
fn simplify(points: &[(f64, f64)], tolerance: f64) -> Vec<(f64, f64)> {
    fn keep(points: &[(f64, f64)], tolerance: f64, out: &mut Vec<(f64, f64)>) {
        let (first, last) = (points[0], points[points.len() - 1]);
        let error = |&(t, v): &(f64, f64)| {
            let line = first.1 + (last.1 - first.1) * (t - first.0) / (last.0 - first.0);
            (v - line).abs()
        };
        let worst = points[1..points.len() - 1]
            .iter()
            .enumerate()
            .max_by(|a, b| error(a.1).total_cmp(&error(b.1)));
        match worst {
            Some((i, p)) if error(p) > tolerance => {
                keep(&points[..=i + 1], tolerance, out);
                keep(&points[i + 1..], tolerance, out);
            }
            _ => out.push(last),
        }
    }
    let mut out = vec![points[0]];
    if points.len() > 1 {
        keep(points, tolerance, &mut out);
    }
    out
}

#[cfg(test)]
mod tests;
