//! Typed analyses and probes: write `op`, `tran`, `dc` and `ac` with the vectors to record, and
//! read their results without writing file or column names.
//!
//! ```no_run
//! # use cuprite_wire::spice::{SpiceNetlist, analysis::{Ac, Tran}};
//! # fn f(netlist: SpiceNetlist) -> Result<(), Box<dyn std::error::Error>> {
//! let deck = netlist.deck();
//! let out = deck.voltage("/OUT")?;
//! let tran = Tran::new(10e-6, 2e-3).probe(&out);
//! let ac = Ac::decade(20, 1.0, 100e3).probe(&out);
//! let sim = deck.analysis(&tran).analysis(&ac).run()?;
//! let v: &[f64] = sim.result(&tran)?.get(&out)?;
//! let gain = sim.result(&ac)?.get(&out)?[0].db();
//! # Ok(()) }
//! ```

use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::spice::{GROUND, SimulationError, SpiceDeck, spice_number};

/// A vector to record in an analysis: a node voltage, the voltage between two nodes, or the
/// current through a voltage source. Get one from [`SpiceDeck::voltage`],
/// [`SpiceDeck::voltage_between`] or [`SpiceDeck::current`].
///
/// A probe names the nodes or the source of the deck it comes from, and does not record the deck.
/// Used in another deck that lacks them, the run fails with ngspice's `no such vector`
/// ([`SimulationError::Ngspice`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Probe {
    expression: String,
}

impl Probe {
    /// The ngspice expression of the vector, e.g. `v("/OUT")` or `i(V1)`, to use in string
    /// commands such as `meas`.
    pub fn expression(&self) -> &str {
        &self.expression
    }

    /// A probe of `expression`, unchecked, for tests.
    #[cfg(test)]
    pub(crate) fn of_expression(expression: &str) -> Self {
        Probe {
            expression: expression.into(),
        }
    }
}

impl fmt::Display for Probe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.expression)
    }
}

/// Why a [`Probe`] cannot be made.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProbeError {
    /// No element of the netlist uses the net.
    UnknownNet(String),
    /// The net is the ground, the reference of the other voltages (0 V; ngspice has no vector
    /// for it).
    Ground(String),
    /// ngspice cannot refer to the node in a command: `$` starts a variable there, even in quotes.
    UnsupportedNode {
        /// The net.
        net: String,
        /// Its SPICE node name.
        node: String,
    },
    /// The netlist has no voltage source (`V...`) of this name, for a current.
    UnknownVoltageSource(String),
    /// The netlist has no independent source (`V...` or `I...`) of this name, for a sweep.
    UnknownSource(String),
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProbeError::UnknownNet(net) => write!(f, "no element uses the net {net:?}"),
            ProbeError::Ground(net) => {
                write!(
                    f,
                    "the net {net:?} is the ground, the reference of the voltages"
                )
            }
            ProbeError::UnsupportedNode { net, node } => write!(
                f,
                "the node {node:?} of the net {net:?} cannot be probed: ngspice reads `$` in \
                 commands as a variable"
            ),
            ProbeError::UnknownVoltageSource(name) => {
                write!(f, "no voltage source named {name:?}")
            }
            ProbeError::UnknownSource(name) => {
                write!(f, "no independent source (`V` or `I`) named {name:?}")
            }
        }
    }
}

impl std::error::Error for ProbeError {}

impl SpiceDeck {
    /// The voltage of `net` against the ground, `v(<node>)`.
    ///
    /// # Errors
    /// If no element uses the net, it is the ground, or its node cannot be probed.
    pub fn voltage(&self, net: &str) -> Result<Probe, ProbeError> {
        Ok(Probe {
            expression: self.node_voltage(net)?,
        })
    }

    /// The voltage of `p` against `n`, `v(<p>)-v(<n>)`. Either may be the ground.
    ///
    /// # Errors
    /// If no element uses one of the nets, both are the ground, or a node cannot be probed.
    pub fn voltage_between(&self, p: &str, n: &str) -> Result<Probe, ProbeError> {
        let expression = match (self.node_voltage(p), self.node_voltage(n)) {
            (Ok(p), Ok(n)) => format!("{p}-{n}"),
            (Ok(p), Err(ProbeError::Ground(_))) => p,
            // A leading `-` would subtract it from the previous argument of `wrdata`.
            (Err(ProbeError::Ground(_)), Ok(n)) => format!("(-{n})"),
            (Err(e), _) | (_, Err(e)) => return Err(e),
        };
        Ok(Probe { expression })
    }

    /// The current through the voltage source `name` (an [`IndependentSource`] or an [`Ammeter`]),
    /// flowing into its positive pin, `i(<name>)`.
    ///
    /// [`IndependentSource`]: crate::spice::IndependentSource
    /// [`Ammeter`]: crate::spice::Ammeter
    ///
    /// # Errors
    /// If the netlist has no voltage source of this name.
    pub fn current(&self, name: &str) -> Result<Probe, ProbeError> {
        match self.netlist().element(name) {
            Some(element) if element.starts_with(['V', 'v']) => Ok(Probe {
                expression: format!("i({element})"),
            }),
            _ => Err(ProbeError::UnknownVoltageSource(name.to_string())),
        }
    }

    /// The independent source `name` (a `V` or `I` element), to sweep with [`Dc`].
    ///
    /// # Errors
    /// If the netlist has no voltage or current source of this name.
    pub fn source(&self, name: &str) -> Result<Source, ProbeError> {
        match self.netlist().element(name) {
            Some(element) if element.starts_with(['V', 'v', 'I', 'i']) => Ok(Source {
                element: element.to_string(),
            }),
            _ => Err(ProbeError::UnknownSource(name.to_string())),
        }
    }

    fn node_voltage(&self, net: &str) -> Result<String, ProbeError> {
        let node = self
            .netlist()
            .node(net)
            .ok_or_else(|| ProbeError::UnknownNet(net.to_string()))?;
        if node == GROUND {
            Err(ProbeError::Ground(net.to_string()))
        } else if node.contains('$') {
            Err(ProbeError::UnsupportedNode {
                net: net.to_string(),
                node: node.to_string(),
            })
        } else {
            Ok(format!("v(\"{node}\")"))
        }
    }
}

/// An independent source of a deck, from [`SpiceDeck::source`].
///
/// Like a [`Probe`], it names the element and does not record the deck: in another deck, it
/// sweeps the element of the same name, or the run fails if there is none.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Source {
    element: String,
}

impl Source {
    /// The element name, e.g. `V1`.
    pub fn element(&self) -> &str {
        &self.element
    }
}

/// A new identity of an analysis, which [`SpiceDeck::analysis`] records with its output file.
fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

mod sealed {
    use crate::spice::analysis::Probe;

    pub trait Sealed {
        /// Changes whenever the analysis changes, so that a result is read only for the analysis
        /// a deck ran.
        fn id(&self) -> u64;
        /// The ngspice command.
        fn command(&self) -> String;
        fn probes(&self) -> &[Probe];
        /// Whether the vectors are complex (written as their real and imaginary parts).
        fn complex(&self) -> bool {
            false
        }
        /// The result from the columns written by `wrdata`: the scale and one column per probe
        /// (two for complex vectors).
        fn read(&self, columns: Vec<Vec<f64>>) -> <Self as super::Analysis>::Output
        where
            Self: super::Analysis + Sized;
    }
}

/// An analysis to add to a deck with [`SpiceDeck::analysis`] and read with
/// [`Simulation::result`](crate::spice::Simulation::result).
pub trait Analysis: sealed::Sealed {
    /// The result of the analysis.
    type Output;
}

/// Probes shared by the analyses, with a new identity for each change.
#[derive(Debug, Clone)]
struct Probes {
    id: u64,
    probes: Vec<Probe>,
}

impl Probes {
    fn new() -> Self {
        Probes {
            id: next_id(),
            probes: Vec::new(),
        }
    }

    fn add(mut self, probes: impl IntoIterator<Item = Probe>) -> Self {
        self.probes.extend(probes);
        self.renew()
    }

    /// The same probes with a new identity, for a change of the analysis.
    fn renew(mut self) -> Self {
        self.id = next_id();
        self
    }
}

macro_rules! analysis_probes {
    ($analysis:ty) => {
        impl $analysis {
            /// Records `probe` in the analysis.
            pub fn probe(self, probe: &Probe) -> Self {
                self.probes([probe.clone()])
            }

            /// Records `probes` in the analysis.
            pub fn probes(mut self, probes: impl IntoIterator<Item = Probe>) -> Self {
                self.probes = self.probes.add(probes);
                self
            }
        }
    };
}

/// The operating point, `op`.
#[derive(Debug, Clone)]
pub struct Op {
    probes: Probes,
}

impl Op {
    /// The operating point without probes.
    pub fn new() -> Self {
        Op {
            probes: Probes::new(),
        }
    }
}

impl Default for Op {
    fn default() -> Self {
        Op::new()
    }
}

analysis_probes!(Op);

impl sealed::Sealed for Op {
    fn id(&self) -> u64 {
        self.probes.id
    }
    fn command(&self) -> String {
        "op".into()
    }
    fn probes(&self) -> &[Probe] {
        &self.probes.probes
    }
    fn read(&self, columns: Vec<Vec<f64>>) -> OpResult {
        // `wrdata` writes the first vector of the plot as the scale of `op`.
        OpResult {
            probes: self.probes.probes.clone(),
            values: columns.into_iter().skip(1).map(|c| c[0]).collect(),
        }
    }
}

impl Analysis for Op {
    type Output = OpResult;
}

/// Whether `value` is finite and above 0.
fn is_positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

/// A transient analysis, `tran <step> <stop> [<start> [<max step>]] [uic]`.
#[derive(Debug, Clone)]
pub struct Tran {
    step: f64,
    stop: f64,
    start: Option<f64>,
    max_step: Option<f64>,
    uic: bool,
    probes: Probes,
}

impl Tran {
    /// From 0 to `stop` seconds, printing every `step` seconds (ngspice also limits its time step
    /// to `step` or to `(stop - start) / 50`, whichever is smaller).
    ///
    /// # Panics
    /// If `step` or `stop` is not finite and positive.
    #[track_caller]
    pub fn new(step: f64, stop: f64) -> Self {
        assert!(
            is_positive(step) && is_positive(stop),
            "the step and the stop time of a transient analysis must be finite and positive: \
             {step}, {stop}"
        );
        Tran {
            step,
            stop,
            start: None,
            max_step: None,
            uic: false,
            probes: Probes::new(),
        }
    }

    /// Records the points from `start` seconds on; the simulation still starts at 0.
    ///
    /// # Panics
    /// If `start` is not finite, is negative, or is not before the stop time (ngspice rejects
    /// it).
    #[track_caller]
    pub fn with_start(mut self, start: f64) -> Self {
        assert!(
            (0.0..self.stop).contains(&start),
            "the start time of a transient analysis must be from 0 to before the stop time {}: \
             {start}",
            self.stop
        );
        self.start = Some(start);
        self.probes = self.probes.renew();
        self
    }

    /// Limits the internal time step of ngspice to `max_step` seconds, instead of the smaller of
    /// `step` and `(stop - start) / 50`.
    ///
    /// # Panics
    /// If `max_step` is not finite and positive.
    #[track_caller]
    pub fn with_max_step(mut self, max_step: f64) -> Self {
        assert!(
            is_positive(max_step),
            "the maximum step of a transient analysis must be finite and positive: {max_step}"
        );
        self.max_step = Some(max_step);
        self.probes = self.probes.renew();
        self
    }

    /// Skips the operating point before the analysis (`uic`): it starts from the initial
    /// conditions of the elements (`IC=`) and of `.ic` directives, and from 0 elsewhere.
    pub fn with_uic(mut self) -> Self {
        self.uic = true;
        self.probes = self.probes.renew();
        self
    }
}

analysis_probes!(Tran);

impl sealed::Sealed for Tran {
    fn id(&self) -> u64 {
        self.probes.id
    }
    fn command(&self) -> String {
        let mut command = format!(
            "tran {} {}",
            spice_number(self.step),
            spice_number(self.stop)
        );
        // The maximum step is the fourth argument, after the start.
        if self.start.is_some() || self.max_step.is_some() {
            command.push_str(&format!(" {}", spice_number(self.start.unwrap_or(0.0))));
        }
        if let Some(max_step) = self.max_step {
            command.push_str(&format!(" {}", spice_number(max_step)));
        }
        if self.uic {
            command.push_str(" uic");
        }
        command
    }
    fn probes(&self) -> &[Probe] {
        &self.probes.probes
    }
    fn read(&self, columns: Vec<Vec<f64>>) -> SweepResult {
        SweepResult::new(self.probes.probes.clone(), columns)
    }
}

impl Analysis for Tran {
    type Output = SweepResult;
}

/// A DC sweep of the value of an independent source, `dc <source> <start> <stop> <step>`.
#[derive(Debug, Clone)]
pub struct Dc {
    source: String,
    start: f64,
    stop: f64,
    step: f64,
    probes: Probes,
}

impl Dc {
    /// Sweeps the value of `source` from `start` to `stop` by `step`.
    ///
    /// # Panics
    /// If a value is not finite, `step` is 0, or `step` goes away from `stop` (ngspice would
    /// write no points).
    #[track_caller]
    pub fn new(source: &Source, start: f64, stop: f64, step: f64) -> Self {
        assert!(
            [start, stop, step].iter().all(|value| value.is_finite())
                && step != 0.0
                && (stop - start) * step >= 0.0,
            "a DC sweep must go from {start} to {stop} by a finite step toward the stop: {step}"
        );
        Dc {
            source: source.element.clone(),
            start,
            stop,
            step,
            probes: Probes::new(),
        }
    }
}

analysis_probes!(Dc);

impl sealed::Sealed for Dc {
    fn id(&self) -> u64 {
        self.probes.id
    }
    fn command(&self) -> String {
        format!(
            "dc {} {} {} {}",
            self.source,
            spice_number(self.start),
            spice_number(self.stop),
            spice_number(self.step)
        )
    }
    fn probes(&self) -> &[Probe] {
        &self.probes.probes
    }
    fn read(&self, columns: Vec<Vec<f64>>) -> SweepResult {
        SweepResult::new(self.probes.probes.clone(), columns)
    }
}

impl Analysis for Dc {
    type Output = SweepResult;
}

/// A small-signal AC analysis, `ac <dec|oct|lin> <points> <start> <stop>`.
#[derive(Debug, Clone)]
pub struct Ac {
    variation: &'static str,
    points: u32,
    start: f64,
    stop: f64,
    probes: Probes,
}

impl Ac {
    #[track_caller]
    fn new(variation: &'static str, points: u32, start: f64, stop: f64) -> Self {
        // ngspice rejects a start of 0 for `dec` and `oct`, and takes default values for a stop
        // before the start.
        let lowest_start = if variation == "lin" {
            0.0
        } else {
            f64::MIN_POSITIVE
        };
        assert!(
            points > 0 && start >= lowest_start && stop.is_finite() && start <= stop,
            "an AC analysis ({variation}) needs points and frequencies from {} up to a finite \
             stop: {points} points from {start} to {stop}",
            if variation == "lin" { "0" } else { "above 0" }
        );
        Ac {
            variation,
            points,
            start,
            stop,
            probes: Probes::new(),
        }
    }

    /// `points` frequencies per decade from `start` to `stop` hertz.
    ///
    /// # Panics
    /// If `points` is 0, `start` is not positive, or `stop` is not finite or is below `start`.
    #[track_caller]
    pub fn decade(points: u32, start: f64, stop: f64) -> Self {
        Ac::new("dec", points, start, stop)
    }

    /// `points` frequencies per octave from `start` to `stop` hertz.
    ///
    /// # Panics
    /// As [`Ac::decade`].
    #[track_caller]
    pub fn octave(points: u32, start: f64, stop: f64) -> Self {
        Ac::new("oct", points, start, stop)
    }

    /// `points` frequencies in all, evenly spaced from `start` to `stop` hertz.
    ///
    /// # Panics
    /// If `points` is 0, `start` is negative, or `stop` is not finite or is below `start`.
    #[track_caller]
    pub fn linear(points: u32, start: f64, stop: f64) -> Self {
        Ac::new("lin", points, start, stop)
    }
}

analysis_probes!(Ac);

impl sealed::Sealed for Ac {
    fn id(&self) -> u64 {
        self.probes.id
    }
    fn command(&self) -> String {
        format!(
            "ac {} {} {} {}",
            self.variation,
            self.points,
            spice_number(self.start),
            spice_number(self.stop)
        )
    }
    fn probes(&self) -> &[Probe] {
        &self.probes.probes
    }
    fn complex(&self) -> bool {
        true
    }
    fn read(&self, columns: Vec<Vec<f64>>) -> AcResult {
        let mut columns = columns.into_iter();
        let frequency = columns.next().unwrap_or_default();
        let mut values = Vec::new();
        while let (Some(re), Some(im)) = (columns.next(), columns.next()) {
            values.push(
                re.into_iter()
                    .zip(im)
                    .map(|(re, im)| Complex { re, im })
                    .collect(),
            );
        }
        AcResult {
            probes: self.probes.probes.clone(),
            frequency,
            values,
        }
    }
}

impl Analysis for Ac {
    type Output = AcResult;
}

/// The `wrdata` command recording the probes of `analysis` into `file`, if it has probes.
pub(crate) fn wrdata_command(analysis: &dyn sealed::Sealed, file: &str) -> Option<String> {
    let probes = analysis.probes();
    if probes.is_empty() {
        return None;
    }
    let mut command = format!("wrdata {file}");
    for probe in probes {
        if analysis.complex() {
            command.push_str(&format!(" real({probe}) imag({probe})"));
        } else {
            command.push_str(&format!(" {probe}"));
        }
    }
    Some(command)
}

pub(crate) use sealed::Sealed as AnalysisCommand;

/// The number of columns `wrdata` writes for `analysis`: the scale and its probes.
pub(crate) fn column_count(analysis: &dyn sealed::Sealed) -> usize {
    1 + analysis.probes().len() * if analysis.complex() { 2 } else { 1 }
}

fn missing_probe(probe: &Probe) -> SimulationError {
    SimulationError::Analysis(format!("the analysis: {probe} is not one of its probes"))
}

fn index_of(probes: &[Probe], probe: &Probe) -> Result<usize, SimulationError> {
    probes
        .iter()
        .position(|p| p == probe)
        .ok_or_else(|| missing_probe(probe))
}

/// The values of the probes at the operating point.
#[derive(Debug, Clone, PartialEq)]
pub struct OpResult {
    probes: Vec<Probe>,
    values: Vec<f64>,
}

impl OpResult {
    /// The value of `probe`.
    ///
    /// # Errors
    /// [`SimulationError::Analysis`] if `probe` is not a probe of the analysis.
    pub fn get(&self, probe: &Probe) -> Result<f64, SimulationError> {
        Ok(self.values[index_of(&self.probes, probe)?])
    }
}

/// The values of the probes over a sweep: time for [`Tran`], the source value for [`Dc`].
#[derive(Debug, Clone, PartialEq)]
pub struct SweepResult {
    probes: Vec<Probe>,
    scale: Vec<f64>,
    values: Vec<Vec<f64>>,
}

impl SweepResult {
    fn new(probes: Vec<Probe>, columns: Vec<Vec<f64>>) -> Self {
        let mut columns = columns.into_iter();
        SweepResult {
            probes,
            scale: columns.next().unwrap_or_default(),
            values: columns.collect(),
        }
    }

    /// The time or the swept value of each point.
    pub fn scale(&self) -> &[f64] {
        &self.scale
    }

    /// The values of `probe` at the points of [`Self::scale`].
    ///
    /// # Errors
    /// [`SimulationError::Analysis`] if `probe` is not a probe of the analysis.
    pub fn get(&self, probe: &Probe) -> Result<&[f64], SimulationError> {
        Ok(&self.values[index_of(&self.probes, probe)?])
    }

    /// The value of `probe` at `x` on the scale, interpolated linearly (`None` outside the scale).
    ///
    /// # Errors
    /// [`SimulationError::Analysis`] if `probe` is not a probe of the analysis.
    pub fn interpolate(&self, probe: &Probe, x: f64) -> Result<Option<f64>, SimulationError> {
        Ok(crate::spice::simulation::interpolate(
            &self.scale,
            self.get(probe)?,
            x,
        ))
    }
}

/// A complex value of an AC analysis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Complex {
    /// The real part.
    pub re: f64,
    /// The imaginary part.
    pub im: f64,
}

impl Complex {
    /// The magnitude.
    pub fn norm(self) -> f64 {
        self.re.hypot(self.im)
    }

    /// The magnitude in decibels, `20 log10 |z|` (as `vdb()` of ngspice).
    pub fn db(self) -> f64 {
        20.0 * self.norm().log10()
    }

    /// The phase in radians, from -π to π (as `vp()` and `ph()` of ngspice).
    pub fn phase(self) -> f64 {
        self.im.atan2(self.re)
    }

    /// The phase in degrees, from -180 to 180.
    pub fn phase_degrees(self) -> f64 {
        self.phase().to_degrees()
    }
}

/// The complex values of the probes over the frequencies of an AC analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct AcResult {
    probes: Vec<Probe>,
    frequency: Vec<f64>,
    values: Vec<Vec<Complex>>,
}

impl AcResult {
    /// The frequencies in hertz.
    pub fn frequency(&self) -> &[f64] {
        &self.frequency
    }

    /// The values of `probe` at [`Self::frequency`].
    ///
    /// # Errors
    /// [`SimulationError::Analysis`] if `probe` is not a probe of the analysis.
    pub fn get(&self, probe: &Probe) -> Result<&[Complex], SimulationError> {
        Ok(&self.values[index_of(&self.probes, probe)?])
    }

    /// The phase of `probe` in radians at [`Self::frequency`], without the jumps of 2π of
    /// [`Complex::phase`] (as `cph()` of ngspice): each step from one frequency to the next is
    /// taken between -π and π.
    ///
    /// # Errors
    /// [`SimulationError::Analysis`] if `probe` is not a probe of the analysis.
    pub fn continuous_phase(&self, probe: &Probe) -> Result<Vec<f64>, SimulationError> {
        let mut phase = Vec::new();
        let mut previous: Option<f64> = None;
        for value in self.get(probe)? {
            let wrapped = value.phase();
            let continuous = match previous {
                None => wrapped,
                Some(previous) => {
                    let step = wrapped - previous;
                    let turns = (step / std::f64::consts::TAU).round();
                    previous + step - turns * std::f64::consts::TAU
                }
            };
            phase.push(continuous);
            previous = Some(continuous);
        }
        Ok(phase)
    }

    /// The group delay of `probe` in seconds at [`Self::frequency`], `-dφ/dω` of
    /// [`Self::continuous_phase`] over the angular frequency ω = 2πf.
    ///
    /// The derivative is taken by finite differences: of second order between the neighbors of a
    /// frequency, of first order at the first and the last ones, so it needs enough points per
    /// decade where the phase turns quickly. With fewer than 2 frequencies, it is NaN.
    ///
    /// # Errors
    /// [`SimulationError::Analysis`] if `probe` is not a probe of the analysis.
    pub fn group_delay(&self, probe: &Probe) -> Result<Vec<f64>, SimulationError> {
        let phase = self.continuous_phase(probe)?;
        let omega: Vec<f64> = self
            .frequency
            .iter()
            .map(|f| std::f64::consts::TAU * f)
            .collect();
        let n = phase.len();
        if n < 2 {
            return Ok(vec![f64::NAN; n]);
        }
        let slope = |i: usize, j: usize| (phase[j] - phase[i]) / (omega[j] - omega[i]);
        Ok((0..n)
            .map(|i| {
                let derivative = if i == 0 {
                    slope(0, 1)
                } else if i == n - 1 {
                    slope(n - 2, n - 1)
                } else {
                    // The slopes on both sides weighted by the width of the other side.
                    let (h0, h1) = (omega[i] - omega[i - 1], omega[i + 1] - omega[i]);
                    (slope(i - 1, i) * h1 + slope(i, i + 1) * h0) / (h0 + h1)
                };
                -derivative
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    fn polar(phase: f64) -> Complex {
        Complex {
            re: phase.cos(),
            im: phase.sin(),
        }
    }

    fn probe(expression: &str) -> Probe {
        Probe::of_expression(expression)
    }

    /// A delay of 1 ms (φ = -ωτ, wrapped several times) on a linear scale.
    #[test]
    fn test_continuous_phase_and_group_delay() {
        let out = probe("v(out)");
        let frequency: Vec<f64> = (0..=40).map(|i| 50.0 * f64::from(i)).collect();
        let values = frequency.iter().map(|f| polar(-TAU * f * 1e-3)).collect();
        let result = AcResult {
            probes: vec![out.clone()],
            frequency: frequency.clone(),
            values: vec![values],
        };
        let phase = result.continuous_phase(&out).unwrap();
        for (f, phase) in frequency.iter().zip(&phase) {
            assert!((phase + TAU * f * 1e-3).abs() < 1e-9, "{f}: {phase}");
        }
        for delay in result.group_delay(&out).unwrap() {
            assert!((delay - 1e-3).abs() < 1e-12, "{delay}");
        }
    }

    /// The second-order difference is exact for a quadratic phase on an uneven scale.
    #[test]
    fn test_group_delay_uneven_scale() {
        let out = probe("v(out)");
        let frequency = vec![1.0, 2.0, 5.0, 6.0, 10.0];
        // φ = -(ω / 100)², so the group delay is 2ω / 100².
        let phase = |f: f64| -(TAU * f / 100.0).powi(2);
        let result = AcResult {
            probes: vec![out.clone()],
            frequency: frequency.clone(),
            values: vec![frequency.iter().map(|&f| polar(phase(f))).collect()],
        };
        let delay = result.group_delay(&out).unwrap();
        for i in 1..frequency.len() - 1 {
            let expected = 2.0 * TAU * frequency[i] / 100.0 / 100.0;
            assert!(
                (delay[i] - expected).abs() < 1e-12,
                "{}: {}",
                frequency[i],
                delay[i]
            );
        }
    }

    #[test]
    fn test_group_delay_single_frequency() {
        let out = probe("v(out)");
        let result = AcResult {
            probes: vec![out.clone()],
            frequency: vec![1.0],
            values: vec![vec![polar(0.5)]],
        };
        assert!(result.group_delay(&out).unwrap()[0].is_nan());
        assert_eq!(result.continuous_phase(&out).unwrap(), [0.5]);
        assert!(matches!(
            result.group_delay(&probe("v(in)")),
            Err(SimulationError::Analysis(_))
        ));
    }

    #[test]
    fn test_phase_degrees() {
        assert_eq!(Complex { re: 0.0, im: -1.0 }.phase_degrees(), -90.0);
        assert_eq!(Complex { re: -1.0, im: 0.0 }.phase_degrees(), 180.0);
    }

    /// Values ngspice rejects, or that cannot be written, are rejected when the analysis is made.
    #[test]
    fn test_invalid_analyses_panic() {
        let source = Source {
            element: "V1".into(),
        };
        let panics =
            |case: &dyn Fn()| std::panic::catch_unwind(std::panic::AssertUnwindSafe(case)).is_err();
        assert!(panics(&|| drop(Tran::new(f64::NAN, 1.0))));
        assert!(panics(&|| drop(Tran::new(1e-6, 0.0))));
        assert!(panics(&|| drop(Tran::new(1e-6, 1e-3).with_start(1e-3))));
        assert!(panics(&|| drop(Tran::new(1e-6, 1e-3).with_start(-1.0))));
        assert!(panics(&|| drop(
            Tran::new(1e-6, 1e-3).with_max_step(f64::INFINITY)
        )));
        assert!(panics(&|| drop(Dc::new(&source, 0.0, 1.0, 0.0))));
        assert!(panics(&|| drop(Dc::new(&source, 0.0, 1.0, -0.1))));
        assert!(panics(&|| drop(Dc::new(&source, 0.0, f64::NAN, 0.1))));
        assert!(panics(&|| drop(Ac::decade(0, 1.0, 1e3))));
        assert!(panics(&|| drop(Ac::decade(10, 0.0, 1e3))));
        assert!(panics(&|| drop(Ac::octave(10, f64::NAN, 1e3))));
        assert!(panics(&|| drop(Ac::linear(10, 10.0, 1.0))));
        assert!(panics(&|| drop(Ac::linear(10, 0.0, f64::INFINITY))));
        // The edge cases ngspice accepts.
        drop(Ac::linear(5, 0.0, 10.0));
        drop(Ac::decade(5, 10.0, 10.0));
        drop(Dc::new(&source, 1.0, 0.0, -0.1));
        drop(Tran::new(1e-6, 1e-3).with_start(0.0).with_max_step(1e-7));
    }
}
