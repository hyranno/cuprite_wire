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
//! driven from outside with voltages from 0 to 1 (relative to `ref`). [`Edges::weights`] makes
//! them from a sequence of [`Drive`] states and the edges of the model: the curves of the weights
//! that reproduce its `[Rising Waveform]` and `[Falling Waveform]`, or else linear edges from its
//! `[Ramp]`.
//!
//! The code models need the XSPICE extension of ngspice (`analog.cm`), which the usual builds
//! load.
//!
//! ```
//! use cuprite_wire_ibis::{
//!     Corner,
//!     spice::{Drive, Parasitics, subcircuit},
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
//! assert!(subckt.text.contains("\n.subckt buffer_typ pin ref ku kd\n"));
//!
//! // High from 10 ns, low from 30 ns.
//! let edges = subckt.edges.unwrap();
//! let weights = edges.weights(Drive::Low, &[(10e-9, Drive::High), (30e-9, Drive::Low)]);
//! assert_eq!(weights.ku.first(), Some(&(0.0, 0.0)));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod edges;

use std::fmt;

pub use edges::{Drive, Edge, Edges, Weights};

use crate::{Component, Corner, IvTable, Model, ModelType, Pin, TypMinMax};

/// The pin port of a subcircuit.
pub const PIN: &str = "pin";
/// The reference port of a subcircuit: the ground of the model, which all its voltages are
/// relative to. It is not named `gnd`, which ngspice reads as the global ground (node 0) even as
/// a port.
pub const REF: &str = "ref";
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
    /// The ports, in order: [`PIN`] and [`REF`], then [`KU`] and [`KD`] for a driver; [`PIN1`]
    /// and [`PIN2`] for a series model.
    pub ports: Vec<&'static str>,
    /// The edges of a driver; `None` for a receiver.
    pub edges: Option<Edges>,
    /// What of the model was left out, such as waveforms with an unsupported fixture, one per
    /// line.
    pub notes: Vec<String>,
    /// The definition, from `.subckt` to `.ends`, with a newline at the end.
    pub text: String,
}

impl Subcircuit {
    /// Whether it is a driver, with the ports [`KU`] and [`KD`].
    pub fn is_driver(&self) -> bool {
        self.edges.is_some()
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
            ModelType::Input | ModelType::InputEcl | ModelType::Terminator => {
                kind(false, false, false)
            }
            ModelType::Output
            | ModelType::Io
            | ModelType::ThreeState
            | ModelType::OutputEcl
            | ModelType::IoEcl
            | ModelType::ThreeStateEcl => kind(true, true, true),
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
/// A model of type `Input`, `Input_ECL` or `Terminator` becomes a receiver with the ports
/// `pin ref`: the package, `C_comp` and the clamps, and for a terminator `[Rgnd]` to
/// `[GND Clamp Reference]`, `[Rpower]` to `[POWER Clamp Reference]` (or else `[Voltage Range]`),
/// and `[Rac]` in series with `[Cac]` to `[GND Clamp Reference]`. The types that drive (`Output`,
/// `I/O`, `3-state`, the open drain, sink and source types, and their ECL versions) become drivers
/// with the ports `pin ref ku kd`, which add `[Pullup]` and `[Pulldown]`. An `I/O` model with `ku`
/// and `kd` at 0 is its receiver.
///
/// The voltage of `[Pullup]` and `[POWER Clamp]` is `[Pullup Reference]` and
/// `[POWER Clamp Reference]`, or else `[Voltage Range]`; that of `[Pulldown]` and `[GND Clamp]` is
/// `[Pulldown Reference]` and `[GND Clamp Reference]`, or else 0. In ECL models, `[Pulldown]` is
/// relative to the supply like `[Pullup]` (`[Pulldown Reference]`, or else `[Voltage Range]`,
/// minus the voltage of the die), as the IBIS specification defines. Beyond the voltages of a
/// table, `pwl` extrapolates its end segments, as in KIBIS.
///
/// The edges of a driver come from its waveforms, as the IBIS specification describes: at each
/// time of a waveform, the weights are those with which the tables, the clamps and `C_comp` drive
/// the current that flows into the fixture (`R_fixture` to `V_fixture`, and `C_fixture`). Of two
/// or more waveforms of an edge, the two with the fixture voltages furthest apart give both
/// weights; a single one gives them with `ku + kd = 1` (or the weight of the only table of an
/// open drain or source model). The weights are kept between 0 and 1, start and end at the levels
/// of the edge, and keep their last value where the equations have no solution. Waveforms with
/// `L_fixture`, `R_dut`, `L_dut` or `C_dut` other than 0 are not used, and listed in
/// [`Subcircuit::notes`]. An edge without usable waveforms is linear, from `[Ramp]`
/// ([`Edges::from_ramp`]).
///
/// # Errors
/// If the model type is not one of the above (the `*_diff`, `Series` and `Series_switch` types;
/// [`series_subcircuit`] writes `Series` models), a driver lacks the I-V table its type needs or
/// an edge lacks both waveforms and `[Ramp]`, a table has fewer than two rows or repeats a
/// voltage, a table or a termination lacks its reference voltage, or a terminator has only one of
/// `[Rac]` and `[Cac]`.
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
    let mut ports = vec![PIN, REF];
    if kind.driver {
        ports.extend([KU, KD]);
    }

    let supply = model.voltage_range.map(|v| v.get(corner));
    let reference = |value: Option<TypMinMax<f64>>, default: Option<f64>| {
        value.map(|v| v.get(corner)).or(default)
    };
    let ecl = matches!(
        model.model_type,
        ModelType::OutputEcl | ModelType::IoEcl | ModelType::ThreeStateEcl
    );
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
            reference(
                model.pulldown_reference,
                if ecl { supply } else { Some(0.0) },
            ),
            ecl,
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
    let mut curves = Vec::new();
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
        curves.push(IvCurve {
            id,
            xs,
            ys,
            reference,
            from_reference,
            weight,
        });
    }
    let c_comp = model.c_comp.map(|c| c.get(corner));
    // The specification allows the terminations only in terminators.
    let terminator = model.model_type == ModelType::Terminator;
    let value = |v: Option<TypMinMax<f64>>| v.filter(|_| terminator).map(|v| v.get(corner));
    let terminations = [
        (
            "RGND",
            "[Rgnd]",
            value(model.rgnd),
            "[GND Clamp Reference]",
            reference(model.gnd_clamp_reference, Some(0.0)),
        ),
        (
            "RPOWER",
            "[Rpower]",
            value(model.rpower),
            "[POWER Clamp Reference]",
            reference(model.power_clamp_reference, supply),
        ),
    ];
    let ac = match (value(model.rac), value(model.cac)) {
        (Some(r), Some(c)) => Some((r, c)),
        (None, None) => None,
        _ => return Err(error("[Rac] and [Cac] go together".into())),
    };
    for (_, keyword, r, reference_keyword, reference) in &terminations {
        if r.is_some() && reference.is_none() {
            return Err(error(format!(
                "{keyword} needs {reference_keyword} or [Voltage Range]"
            )));
        }
    }

    let mut notes = Vec::new();
    let edges = if kind.driver {
        let weighted = |weight| curves.iter().find(|c| c.weight == Some(weight));
        let buffer = edges::Buffer {
            pullup: weighted(KU),
            pulldown: weighted(KD),
            clamps: curves.iter().filter(|c| c.weight.is_none()).collect(),
            c_comp: c_comp.unwrap_or(0.0),
        };
        let mut edge = |waveforms, rising| {
            edges::from_waveforms(&buffer, waveforms, rising, corner, &mut notes)
        };
        let rising = edge(&model.rising_waveforms, true);
        let falling = edge(&model.falling_waveforms, false);
        let ramp = |keyword: &str| {
            let ramp = model.ramp.as_ref().ok_or_else(|| {
                error(format!(
                    "a `{}` model needs [{keyword} Waveform] or [Ramp]",
                    model.model_type
                ))
            })?;
            Ok(Edges::from_ramp(ramp, corner))
        };
        Some(Edges {
            rising: match rising {
                Some(edge) => edge,
                None => ramp("Rising")?.rising,
            },
            falling: match falling {
                Some(edge) => edge,
                None => ramp("Falling")?.falling,
            },
        })
    } else {
        None
    };

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
    for note in &notes {
        line(&mut text, format_args!("* {note}"));
    }
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
        line(&mut text, format_args!("CPKG {PIN} {REF} {}", num(c)));
    }
    if let Some(c) = c_comp {
        line(&mut text, format_args!("CCOMP {die} {REF} {}", num(c)));
    }

    // A node at the voltage of a reference: `ref` itself, or a source named after the element.
    let gnd_reference = terminations[0].4.unwrap_or(0.0);
    let reference_node = |text: &mut String, id: &str, voltage: f64| {
        if voltage == 0.0 {
            REF.to_string()
        } else {
            let node = format!("{}_ref", id.to_ascii_lowercase());
            line(text, format_args!("V{id} {node} {REF} {}", num(voltage)));
            node
        }
    };
    // The terminations of a terminator.
    for (id, _, r, _, reference) in terminations {
        if let (Some(r), Some(reference)) = (r, reference) {
            let node = reference_node(&mut text, id, reference);
            line(&mut text, format_args!("{id} {die} {node} {}", num(r)));
        }
    }
    if let Some((r, c)) = ac {
        let node = reference_node(&mut text, "AC", gnd_reference);
        line(&mut text, format_args!("RAC {die} ac {}", num(r)));
        line(&mut text, format_args!("CAC ac {node} {}", num(c)));
    }

    for curve in &curves {
        let id = curve.id;
        let ref_node = reference_node(&mut text, id, curve.reference);
        // The current into the pin flows from the die to the reference.
        let input = if curve.from_reference {
            format!("%vd({ref_node} {die})")
        } else {
            format!("%vd({die} {ref_node})")
        };
        match curve.weight {
            None => line(
                &mut text,
                format_args!("A{id} {input} %id({die} {ref_node}) {id}"),
            ),
            Some(weight) => {
                // The table current as a voltage, multiplied by the weight.
                let current = format!("i_{}", id.to_ascii_lowercase());
                line(
                    &mut text,
                    format_args!("A{id} {input} %vd({current} {REF}) {id}"),
                );
                line(
                    &mut text,
                    format_args!("B{id} {die} {ref_node} I=v({current},{REF})*v({weight},{REF})"),
                );
            }
        }
        line(
            &mut text,
            format_args!(
                ".model {id} pwl(x_array=[{}] y_array=[{}] input_domain={PWL_DOMAIN} fraction=TRUE)",
                join(&curve.xs),
                join(&curve.ys)
            ),
        );
    }
    line(&mut text, format_args!(".ends {name}"));

    Ok(Subcircuit {
        name: name.to_string(),
        ports,
        edges,
        notes,
        text,
    })
}

/// The first port of a series subcircuit ([`series_subcircuit`]).
pub const PIN1: &str = "pin1";
/// The second port of a series subcircuit.
pub const PIN2: &str = "pin2";

/// Writes a `Series` model as a subcircuit named `name` with the ports [`PIN1`] and [`PIN2`], with
/// the values of the `corner`: its passive elements in parallel between the ports
/// ([`SeriesElements`](crate::SeriesElements)). The packages of the two pins are not part of it.
///
/// # Errors
/// If the model is not of type `Series`, has `[Series Current]` or `[Series MOSFET]`, or has none
/// of `[R Series]`, `[L Series]` and `[C Series]`.
pub fn series_subcircuit(
    name: &str,
    model: &Model,
    corner: Corner,
) -> Result<Subcircuit, SubcircuitError> {
    let error = |message: &str| SubcircuitError {
        model: model.name.clone(),
        message: message.to_string(),
    };
    if model.model_type != ModelType::Series {
        return Err(error(&format!(
            "a series subcircuit needs a `Series` model, not `{}`",
            model.model_type
        )));
    }
    let series = &model.series;
    if series.nonlinear {
        return Err(error(
            "[Series Current] and [Series MOSFET] are not supported",
        ));
    }
    let value = |v: Option<TypMinMax<f64>>| v.map(|v| v.get(corner));
    let mut text = format!(
        "* IBIS model {} ({}), corner {corner:?}\n.subckt {name} {PIN1} {PIN2}\n",
        model.name, model.model_type
    );
    let mut empty = true;
    if let Some(r) = value(series.r) {
        text.push_str(&format!("RS {PIN1} {PIN2} {}\n", num(r)));
        empty = false;
    }
    // A branch of an element in series with optional others, from PIN1 to PIN2.
    let mut branch = |id: &str, elements: [(char, Option<f64>); 3]| {
        let elements: Vec<_> = elements
            .into_iter()
            .filter_map(|(kind, v)| v.map(|v| (kind, v)))
            .collect();
        for (i, (kind, v)) in elements.iter().enumerate() {
            let from = if i == 0 {
                PIN1.to_string()
            } else {
                format!("{}{i}", id.to_ascii_lowercase())
            };
            let to = if i + 1 == elements.len() {
                PIN2.to_string()
            } else {
                format!("{}{}", id.to_ascii_lowercase(), i + 1)
            };
            text.push_str(&format!("{kind}{id}{i} {from} {to} {}\n", num(*v)));
        }
    };
    if let Some(l) = value(series.l) {
        branch("L", [('L', Some(l)), ('R', value(series.rl)), ('R', None)]);
        empty = false;
    }
    if let Some(c) = value(series.c) {
        branch(
            "C",
            [
                ('C', Some(c)),
                ('L', value(series.lc)),
                ('R', value(series.rc)),
            ],
        );
        empty = false;
    }
    if empty {
        return Err(error(
            "a `Series` model needs [R Series], [L Series] or [C Series]",
        ));
    }
    text.push_str(&format!(".ends {name}\n"));
    Ok(Subcircuit {
        name: name.to_string(),
        ports: vec![PIN1, PIN2],
        edges: None,
        notes: Vec::new(),
        text,
    })
}

/// The `input_domain` of the `pwl` code models: the fraction of a segment around each row of a
/// table within which they round the corner (as KIBIS does).
const PWL_DOMAIN: f64 = 0.05;

/// An I-V table of a model at a corner, with its reference.
struct IvCurve {
    /// The name of its elements (`PU`).
    id: &'static str,
    /// The voltages, increasing.
    xs: Vec<f64>,
    /// The currents into the pin.
    ys: Vec<f64>,
    /// The voltage of the reference, relative to the port `ref`.
    reference: f64,
    /// Whether the voltages are relative to the reference (`reference - die`) rather than to
    /// ground (`die - reference`).
    from_reference: bool,
    /// The port weighting the current.
    weight: Option<&'static str>,
}

impl IvCurve {
    /// The current into the pin at the voltage of the die, as `pwl` gives it: linear between the
    /// rows and beyond the ends, and quadratic within [`PWL_DOMAIN`] of the shorter adjacent
    /// segment around each inner row, joining the slopes on both sides.
    fn current(&self, die: f64) -> f64 {
        let x = if self.from_reference {
            self.reference - die
        } else {
            die - self.reference
        };
        let (xs, ys) = (&self.xs, &self.ys);
        let slope = |i: usize| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]);
        let i = xs.partition_point(|&v| v <= x).clamp(1, xs.len() - 1);
        // The row nearest to `x`, if it is an inner one.
        let near = if x - xs[i - 1] < xs[i] - x { i - 1 } else { i };
        if near > 0 && near < xs.len() - 1 {
            let domain = PWL_DOMAIN * (xs[near] - xs[near - 1]).min(xs[near + 1] - xs[near]);
            let u = x - (xs[near] - domain);
            if (0.0..2.0 * domain).contains(&u) {
                let (lower, upper) = (slope(near - 1), slope(near));
                return ys[near] - lower * domain
                    + lower * u
                    + (upper - lower) / (4.0 * domain) * u * u;
            }
        }
        ys[i - 1] + slope(i - 1) * (x - xs[i - 1])
    }
}

/// The voltages and currents of a table at the corner, in increasing voltage.
fn table_points(table: &IvTable, corner: Corner) -> Result<(Vec<f64>, Vec<f64>), String> {
    let mut points = table.curve(corner);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_num() {
        assert_eq!(num(5.5e-2), "5.5e-2");
        assert_eq!(num(3.3), "3.3e0");
        assert_eq!(num(0.0), "0e0");
    }
}
