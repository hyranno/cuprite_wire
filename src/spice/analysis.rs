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
    }
}

/// An analysis to add to a deck with [`SpiceDeck::analysis`] and read with
/// [`Simulation::result`](crate::spice::Simulation::result).
pub trait Analysis: sealed::Sealed {
    /// The result of the analysis.
    type Output;
    #[doc(hidden)]
    /// The result from the columns written by `wrdata`: the scale and one column per probe
    /// (two for complex vectors).
    fn read(&self, columns: Vec<Vec<f64>>) -> Self::Output;
}

/// Probes shared by the analyses, with a new identity for each change.
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
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
}

impl Analysis for Op {
    type Output = OpResult;
    fn read(&self, columns: Vec<Vec<f64>>) -> OpResult {
        // `wrdata` writes the first vector of the plot as the scale of `op`.
        OpResult {
            probes: self.probes.probes.clone(),
            values: columns.into_iter().skip(1).map(|c| c[0]).collect(),
        }
    }
}

/// A transient analysis, `tran <step> <stop>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Tran {
    step: f64,
    stop: f64,
    probes: Probes,
}

impl Tran {
    /// From 0 to `stop` seconds, printing every `step` seconds (ngspice also limits its time step
    /// to `step` or to `(stop - 0) / 50`, whichever is smaller).
    pub fn new(step: f64, stop: f64) -> Self {
        Tran {
            step,
            stop,
            probes: Probes::new(),
        }
    }
}

analysis_probes!(Tran);

impl sealed::Sealed for Tran {
    fn id(&self) -> u64 {
        self.probes.id
    }
    fn command(&self) -> String {
        format!(
            "tran {} {}",
            spice_number(self.step),
            spice_number(self.stop)
        )
    }
    fn probes(&self) -> &[Probe] {
        &self.probes.probes
    }
}

impl Analysis for Tran {
    type Output = SweepResult;
    fn read(&self, columns: Vec<Vec<f64>>) -> SweepResult {
        SweepResult::new(self.probes.probes.clone(), columns)
    }
}

/// A DC sweep of the value of an independent source, `dc <source> <start> <stop> <step>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Dc {
    source: String,
    start: f64,
    stop: f64,
    step: f64,
    probes: Probes,
}

impl Dc {
    /// Sweeps the value of `source` from `start` to `stop` by `step`.
    pub fn new(source: &Source, start: f64, stop: f64, step: f64) -> Self {
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
}

impl Analysis for Dc {
    type Output = SweepResult;
    fn read(&self, columns: Vec<Vec<f64>>) -> SweepResult {
        SweepResult::new(self.probes.probes.clone(), columns)
    }
}

/// A small-signal AC analysis, `ac <dec|oct|lin> <points> <start> <stop>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Ac {
    variation: &'static str,
    points: u32,
    start: f64,
    stop: f64,
    probes: Probes,
}

impl Ac {
    fn new(variation: &'static str, points: u32, start: f64, stop: f64) -> Self {
        Ac {
            variation,
            points,
            start,
            stop,
            probes: Probes::new(),
        }
    }

    /// `points` frequencies per decade from `start` to `stop` hertz.
    pub fn decade(points: u32, start: f64, stop: f64) -> Self {
        Ac::new("dec", points, start, stop)
    }

    /// `points` frequencies per octave from `start` to `stop` hertz.
    pub fn octave(points: u32, start: f64, stop: f64) -> Self {
        Ac::new("oct", points, start, stop)
    }

    /// `points` frequencies in all, evenly spaced from `start` to `stop` hertz.
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
}

impl Analysis for Ac {
    type Output = AcResult;
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
    SimulationError::Analysis(format!("{probe} is not a probe of the analysis"))
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

    /// The phase in radians (as `vp()` of ngspice).
    pub fn phase(self) -> f64 {
        self.im.atan2(self.re)
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
}
