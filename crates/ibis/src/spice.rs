//! SPICE subcircuits of [`Model`]s, for ngspice.
//!
//! [`subcircuit`] writes a model, with the package parasitics of a pin, as a `.subckt`. The
//! structure follows the IBIS conversion of KiCad (KIBIS), so that the results can be compared:
//!
//! - the package: `R_pkg` and `L_pkg` in series from the pin to the die, and `C_pkg` from the pin
//!   to ground;
//! - `C_comp` from the die to ground;
//! - the I-V tables as XSPICE `pwl` code models (`[GND Clamp]`, `[POWER Clamp]`, and `[Pulldown]`
//!   and `[Pullup]` of a driver), each against a voltage source of its reference;
//! - the current of `[Pullup]` multiplied by the voltage of the port `ku`, and that of
//!   `[Pulldown]` by `kd`.
//!
//! Unlike KIBIS, the stimulus is not part of the subcircuit: a driver has the ports `ku` and `kd`,
//! driven from outside with voltages from 0 to 1 (relative to `gnd`). [`EdgeTimes::weights`] makes
//! them from a sequence of [`Drive`] states and the `[Ramp]` of the model.
//!
//! The code models need the XSPICE extension of ngspice (`analog.cm`), which the usual builds
//! load.
//!
//! ```
//! use cuprite_wire_ibis::{
//!     Corner,
//!     spice::{Drive, EdgeTimes, Parasitics, subcircuit},
//! };
//!
//! let ibs = cuprite_wire_ibis::parse(
//!     "\
//! [IBIS Ver] 2.1
//! [File Name] buffer.ibs
//! [Component] Example
//! [Manufacturer] Nobody
//! [Package]
//! R_pkg 50m NA NA
//! L_pkg 2n NA NA
//! C_pkg 1p NA NA
//! [Pin] signal_name model_name
//! 1 OUT buffer
//! [Model] buffer
//! Model_type Output
//! C_comp 2p NA NA
//! [Voltage Range] 3.3 3.0 3.6
//! [Pulldown]
//! 0 0 NA NA
//! 3.3 50m NA NA
//! [Pullup]
//! 0 0 NA NA
//! 3.3 -50m NA NA
//! [Ramp]
//! dV/dt_r 2.0/1n NA NA
//! dV/dt_f 2.0/1n NA NA
//! [END]
//! ",
//! )?;
//! let component = ibs.component("Example").unwrap();
//! let pin = component.pin("1").unwrap();
//! let parasitics = Parasitics::of_pin(component, pin, Corner::Typ);
//! let model = ibs.model("buffer").unwrap();
//! let subckt = subcircuit("buffer_typ", model, &parasitics, Corner::Typ)?;
//! assert!(subckt.text.contains("\n.subckt buffer_typ pin gnd ku kd\n"));
//!
//! // High from 10 ns, low from 30 ns.
//! let edges = subckt.edge_times.unwrap();
//! let weights = edges.weights(Drive::Low, &[(10e-9, Drive::High), (30e-9, Drive::Low)]);
//! assert_eq!(weights.ku.first(), Some(&(0.0, 0.0)));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;

use crate::{Component, Corner, IvTable, Model, ModelType, Pin, Ramp, TypMinMax};

/// The pin port of a subcircuit.
pub const PIN: &str = "pin";
/// The ground port of a subcircuit, the reference of all voltages of the model.
pub const GND: &str = "gnd";
/// The port of a driver weighting `[Pullup]`: the voltage from 0 (off) to 1 (on).
pub const KU: &str = "ku";
/// The port of a driver weighting `[Pulldown]`.
pub const KD: &str = "kd";

/// The package parasitics between a pin and the die. `None` leaves the element out.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[non_exhaustive]
pub struct Parasitics {
    /// The series resistance, in ohms.
    pub r: Option<f64>,
    /// The series inductance, in henries.
    pub l: Option<f64>,
    /// The capacitance from the pin to ground, in farads.
    pub c: Option<f64>,
}

impl Parasitics {
    /// The parasitics of no package: the pin is the die.
    pub const NONE: Parasitics = Parasitics {
        r: None,
        l: None,
        c: None,
    };

    /// The parasitics with the values.
    pub fn new(r: Option<f64>, l: Option<f64>, c: Option<f64>) -> Self {
        Parasitics { r, l, c }
    }

    /// The parasitics of a pin of the component: `R_pin`, `L_pin` and `C_pin` of the pin, or
    /// else the corner of `R_pkg`, `L_pkg` and `C_pkg` of the component.
    pub fn of_pin(component: &Component, pin: &Pin, corner: Corner) -> Self {
        let package = |value: Option<TypMinMax<f64>>| value.map(|v| v.get(corner));
        Parasitics {
            r: pin.r_pin.or(package(component.package.r_pkg)),
            l: pin.l_pin.or(package(component.package.l_pkg)),
            c: pin.c_pin.or(package(component.package.c_pkg)),
        }
    }
}

/// A `.subckt` written by [`subcircuit`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Subcircuit {
    /// The name of the subcircuit.
    pub name: String,
    /// The ports, in order: [`PIN`] and [`GND`], then [`KU`] and [`KD`] for a driver.
    pub ports: Vec<&'static str>,
    /// The edge times of a driver, from its `[Ramp]`; `None` for a receiver.
    pub edge_times: Option<EdgeTimes>,
    /// The definition, from `.subckt` to `.ends`, with a newline at the end.
    pub text: String,
}

impl Subcircuit {
    /// Whether it is a driver, with the ports [`KU`] and [`KD`].
    pub fn is_driver(&self) -> bool {
        self.edge_times.is_some()
    }
}

/// The reason a model has no subcircuit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubcircuitError {
    /// The name of the model.
    pub model: String,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for SubcircuitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "model `{}`: {}", self.model, self.message)
    }
}

impl std::error::Error for SubcircuitError {}

/// What a model type has.
struct Kind {
    /// Whether it drives the pin (the ports `ku` and `kd`).
    driver: bool,
    /// The I-V tables it must have to drive.
    needs_pullup: bool,
    needs_pulldown: bool,
}

impl Kind {
    fn of(model_type: ModelType) -> Option<Kind> {
        let kind = |driver, needs_pullup, needs_pulldown| {
            Some(Kind {
                driver,
                needs_pullup,
                needs_pulldown,
            })
        };
        match model_type {
            ModelType::Input => kind(false, false, false),
            ModelType::Output | ModelType::Io | ModelType::ThreeState => kind(true, true, true),
            ModelType::OpenDrain
            | ModelType::IoOpenDrain
            | ModelType::OpenSink
            | ModelType::IoOpenSink => kind(true, false, true),
            ModelType::OpenSource | ModelType::IoOpenSource => kind(true, true, false),
            _ => None,
        }
    }
}

/// Writes the model as a subcircuit named `name`, with the package `parasitics` and the values of
/// the `corner`.
///
/// A model of type `Input` becomes a receiver with the ports `pin gnd`: the package, `C_comp` and
/// the clamps. The types that drive (`Output`, `I/O`, `3-state` and the open drain, sink and
/// source types) become drivers with the ports `pin gnd ku kd`, which add `[Pullup]` and
/// `[Pulldown]`. An `I/O` model with `ku` and `kd` at 0 is its receiver.
///
/// The voltage of `[Pullup]` and `[POWER Clamp]` is `[Pullup Reference]` and
/// `[POWER Clamp Reference]`, or else `[Voltage Range]`; that of `[Pulldown]` and `[GND Clamp]` is
/// `[Pulldown Reference]` and `[GND Clamp Reference]`, or else 0. Beyond the voltages of a table,
/// `pwl` extrapolates its end segments, as in KIBIS.
///
/// # Errors
/// If the model type is not one of the above (ECL, differential, series and terminator models),
/// a driver lacks the I-V table its type needs or `[Ramp]`, a table has fewer than two rows or
/// repeats a voltage, or a table lacks its reference voltage.
pub fn subcircuit(
    name: &str,
    model: &Model,
    parasitics: &Parasitics,
    corner: Corner,
) -> Result<Subcircuit, SubcircuitError> {
    let error = |message: String| SubcircuitError {
        model: model.name.clone(),
        message,
    };
    let kind = Kind::of(model.model_type).ok_or_else(|| {
        error(format!(
            "the model type `{}` is not supported",
            model.model_type
        ))
    })?;
    if kind.needs_pullup && model.pullup.is_none() {
        return Err(error(format!(
            "a `{}` model needs [Pullup]",
            model.model_type
        )));
    }
    if kind.needs_pulldown && model.pulldown.is_none() {
        return Err(error(format!(
            "a `{}` model needs [Pulldown]",
            model.model_type
        )));
    }
    let edge_times = if kind.driver {
        let ramp = model
            .ramp
            .as_ref()
            .ok_or_else(|| error(format!("a `{}` model needs [Ramp]", model.model_type)))?;
        Some(EdgeTimes::from_ramp(ramp, corner))
    } else {
        None
    };
    let mut ports = vec![PIN, GND];
    if kind.driver {
        ports.extend([KU, KD]);
    }

    let mut text = String::new();
    let line = |text: &mut String, args: fmt::Arguments| {
        text.push_str(&args.to_string());
        text.push('\n');
    };
    line(
        &mut text,
        format_args!(
            "* IBIS model {} ({}), corner {:?}",
            model.name, model.model_type, corner
        ),
    );
    line(
        &mut text,
        format_args!(".subckt {name} {}", ports.join(" ")),
    );

    // The package, from the pin to the die.
    let mut node = PIN;
    if let Some(r) = parasitics.r {
        line(&mut text, format_args!("RPKG {node} pkg {}", num(r)));
        node = "pkg";
    }
    if let Some(l) = parasitics.l {
        line(&mut text, format_args!("LPKG {node} die {}", num(l)));
        node = "die";
    }
    let die = node;
    if let Some(c) = parasitics.c {
        line(&mut text, format_args!("CPKG {PIN} {GND} {}", num(c)));
    }
    if let Some(c) = model.c_comp {
        line(
            &mut text,
            format_args!("CCOMP {die} {GND} {}", num(c.get(corner))),
        );
    }

    let supply = model.voltage_range.map(|v| v.get(corner));
    let reference = |value: Option<TypMinMax<f64>>, default: Option<f64>| {
        value.map(|v| v.get(corner)).or(default)
    };
    // A table, its reference keyword and value, and whether its voltages are relative to the
    // reference (`ref - die`) rather than to ground (`die - ref`).
    let tables = [
        (
            "GC",
            "[GND Clamp]",
            &model.gnd_clamp,
            reference(model.gnd_clamp_reference, Some(0.0)),
            false,
            None,
        ),
        (
            "PC",
            "[POWER Clamp]",
            &model.power_clamp,
            reference(model.power_clamp_reference, supply),
            true,
            None,
        ),
        (
            "PD",
            "[Pulldown]",
            &model.pulldown,
            reference(model.pulldown_reference, Some(0.0)),
            false,
            Some(KD),
        ),
        (
            "PU",
            "[Pullup]",
            &model.pullup,
            reference(model.pullup_reference, supply),
            true,
            Some(KU),
        ),
    ];
    for (id, keyword, table, reference, from_reference, weight) in tables {
        let Some(table) = table else { continue };
        // A receiver ignores the tables of the driver (an `Input` model should not have them).
        if weight.is_some() && !kind.driver {
            continue;
        }
        let reference = reference.ok_or_else(|| {
            error(format!(
                "{keyword} needs its reference voltage or [Voltage Range]"
            ))
        })?;
        let (xs, ys) = table_points(table, corner).map_err(|m| error(format!("{keyword} {m}")))?;
        let ref_node = if reference == 0.0 {
            GND.to_string()
        } else {
            let node = format!("{}_ref", id.to_ascii_lowercase());
            line(
                &mut text,
                format_args!("V{id} {node} {GND} {}", num(reference)),
            );
            node
        };
        // The current into the pin flows from the die to the reference.
        let input = if from_reference {
            format!("%vd({ref_node} {die})")
        } else {
            format!("%vd({die} {ref_node})")
        };
        match weight {
            None => line(
                &mut text,
                format_args!("A{id} {input} %id({die} {ref_node}) {id}"),
            ),
            Some(weight) => {
                // The table current as a voltage, multiplied by the weight.
                let current = format!("i_{}", id.to_ascii_lowercase());
                line(
                    &mut text,
                    format_args!("A{id} {input} %vd({current} {GND}) {id}"),
                );
                line(
                    &mut text,
                    format_args!("B{id} {die} {ref_node} I=v({current},{GND})*v({weight},{GND})"),
                );
            }
        }
        line(
            &mut text,
            format_args!(
                ".model {id} pwl(x_array=[{}] y_array=[{}] input_domain=0.05 fraction=TRUE)",
                join(&xs),
                join(&ys)
            ),
        );
    }
    line(&mut text, format_args!(".ends {name}"));

    Ok(Subcircuit {
        name: name.to_string(),
        ports,
        edge_times,
        text,
    })
}

/// The voltages and currents of a table at the corner, in increasing voltage.
fn table_points(table: &IvTable, corner: Corner) -> Result<(Vec<f64>, Vec<f64>), String> {
    let mut points: Vec<_> = table
        .points
        .iter()
        .map(|p| (p.voltage, p.current.get(corner)))
        .collect();
    if points.len() < 2 {
        return Err("needs at least two rows".into());
    }
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    if let Some(w) = points.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(format!("repeats the voltage {}", w[0].0));
    }
    Ok(points.into_iter().unzip())
}

/// A number for SPICE, exact and without a scale suffix.
fn num(value: f64) -> String {
    format!("{value:e}")
}

fn join(values: &[f64]) -> String {
    values.iter().map(|&v| num(v)).collect::<Vec<_>>().join(" ")
}

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

/// The time a driver takes to switch between low and high.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct EdgeTimes {
    /// The rising edge, in seconds.
    pub rise: f64,
    /// The falling edge, in seconds.
    pub fall: f64,
}

impl EdgeTimes {
    /// The edge times with the values.
    ///
    /// # Panics
    /// If a time is not positive.
    #[track_caller]
    pub fn new(rise: f64, fall: f64) -> Self {
        assert!(
            rise > 0.0 && fall > 0.0,
            "the edge times must be positive: rise {rise}, fall {fall}"
        );
        EdgeTimes { rise, fall }
    }

    /// The edge times of the corner of a `[Ramp]`, as KIBIS takes them: the `dt` of `dV/dt`, the
    /// time from 20% to 80% of the swing, extended to the whole swing (`dt / 0.6`).
    ///
    /// # Panics
    /// If a `dt` is not positive.
    #[track_caller]
    pub fn from_ramp(ramp: &Ramp, corner: Corner) -> Self {
        Self::new(
            ramp.dv_dt_r.get(corner).dt / 0.6,
            ramp.dv_dt_f.get(corner).dt / 0.6,
        )
    }

    /// The piecewise linear waveforms of `ku` and `kd` that start in the state `initial` and
    /// change to the state of each event at its time (in seconds, non-decreasing and not
    /// negative).
    ///
    /// Each change is linear: to `High` over [`rise`](Self::rise), to `Low` over
    /// [`fall`](Self::fall), and to `HiZ` over `fall` from `High` and `rise` from `Low`. A change
    /// that starts before the previous one ends starts from where that one has got to, and takes
    /// its time in proportion to the rest of the way.
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
            let duration = match (state, next) {
                (_, Drive::High) | (Drive::Low, Drive::HiZ) => self.rise,
                (_, Drive::Low) | (Drive::High, Drive::HiZ) => self.fall,
                (Drive::HiZ, Drive::HiZ) => continue,
            };
            let (ku, kd) = next.weights();
            ramp_to(&mut weights.ku, time, ku, duration);
            ramp_to(&mut weights.kd, time, kd, duration);
            state = next;
        }
        weights
    }
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

/// Changes a waveform to `target` from `time`, at the rate of a whole swing in `duration`.
fn ramp_to(points: &mut Vec<(f64, f64)>, time: f64, target: f64, duration: f64) {
    let value = value_at(points, time);
    // The only point after `time` is the end of a change in progress.
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
    points.push((time + duration * (target - value).abs(), target));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn edges() -> EdgeTimes {
        EdgeTimes::new(2.0, 1.0)
    }

    #[test]
    fn test_weights_edges() {
        let w = edges().weights(
            Drive::Low,
            &[(10.0, Drive::High), (20.0, Drive::Low), (30.0, Drive::HiZ)],
        );
        assert_eq!(
            w.ku,
            [
                (0.0, 0.0),
                (10.0, 0.0),
                (12.0, 1.0),
                (20.0, 1.0),
                (21.0, 0.0)
            ]
        );
        // To HiZ from Low takes the rise time.
        assert_eq!(
            w.kd,
            [
                (0.0, 1.0),
                (10.0, 1.0),
                (12.0, 0.0),
                (20.0, 0.0),
                (21.0, 1.0),
                (30.0, 1.0),
                (32.0, 0.0)
            ]
        );
    }

    #[test]
    fn test_weights_at_zero() {
        let w = edges().weights(Drive::HiZ, &[(0.0, Drive::High), (5.0, Drive::HiZ)]);
        assert_eq!(w.ku, [(0.0, 0.0), (2.0, 1.0), (5.0, 1.0), (6.0, 0.0)]);
        assert_eq!(w.kd, [(0.0, 0.0)]);
    }

    #[test]
    fn test_weights_interrupted() {
        // Low at 11, halfway up: back down from 0.5 over half the fall time.
        let w = edges().weights(Drive::Low, &[(10.0, Drive::High), (11.0, Drive::Low)]);
        assert_eq!(w.ku, [(0.0, 0.0), (10.0, 0.0), (11.0, 0.5), (11.5, 0.0)]);
        assert_eq!(w.kd, [(0.0, 1.0), (10.0, 1.0), (11.0, 0.5), (11.5, 1.0)]);
        // An event repeating the state changes nothing.
        let w = edges().weights(Drive::High, &[(1.0, Drive::High), (2.0, Drive::HiZ)]);
        assert_eq!(w.ku, [(0.0, 1.0), (2.0, 1.0), (3.0, 0.0)]);
    }

    #[test]
    #[should_panic(expected = "in order")]
    fn test_weights_out_of_order() {
        edges().weights(Drive::Low, &[(2.0, Drive::High), (1.0, Drive::Low)]);
    }

    #[test]
    fn test_num() {
        assert_eq!(num(5.5e-2), "5.5e-2");
        assert_eq!(num(3.3), "3.3e0");
        assert_eq!(num(0.0), "0e0");
    }
}
