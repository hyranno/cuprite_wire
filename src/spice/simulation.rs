//! Running a deck with ngspice and reading its results.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::spice::SpiceDeck;

/// The ngspice program, run in batch mode (`ngspice -b deck.cir`).
///
/// glibc `tmpfile()`, which the CLI copies its input with, ignores `$TMPDIR`; where `/tmp` is
/// read-only (e.g. the Claude Code sandbox), the devcontainer's `ngspice` wrapper redirects it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ngspice {
    /// The ngspice executable (a name looked up in `PATH`, or a path).
    pub program: PathBuf,
}

impl Default for Ngspice {
    /// `ngspice` from `PATH`.
    fn default() -> Self {
        Ngspice {
            program: "ngspice".into(),
        }
    }
}

impl Ngspice {
    /// Writes the deck into a new temporary directory and runs it there, so that relative
    /// `wrdata` paths land in that directory.
    ///
    /// If the models need a compatibility mode ([`SpiceNetlist::ngbehavior`](crate::spice::SpiceNetlist::ngbehavior)),
    /// it is set with a `.spiceinit` file in that directory.
    ///
    /// # Errors
    /// [`SimulationError::Io`] if the program cannot be run or the run directory cannot be
    /// prepared, and [`SimulationError::Ngspice`] if ngspice fails or prints an error.
    pub fn run(&self, deck: &SpiceDeck) -> Result<Simulation, SimulationError> {
        let dir = prepare_run_dir(deck)?;
        let output = Command::new(&self.program)
            .arg("-b")
            .arg(dir.join(DECK_FILE))
            .current_dir(dir.path())
            .output()?;
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        finish_run(dir, log, output.status.success())
    }
}

impl Simulator for Ngspice {
    fn run(&self, deck: &SpiceDeck) -> Result<Simulation, SimulationError> {
        Ngspice::run(self, deck)
    }
}

/// A way of running decks with ngspice.
pub trait Simulator {
    /// Runs `deck` in a new temporary directory, which the result holds.
    ///
    /// # Errors
    /// The variants of [`SimulationError`] the backend can fail with; see the implementations.
    fn run(&self, deck: &SpiceDeck) -> Result<Simulation, SimulationError>;
}

/// The deck in the run directory, as the CLI runs it: `ngspice -b deck.cir` there reproduces
/// a run of any backend.
pub(crate) const DECK_FILE: &str = "deck.cir";

/// Creates a run directory with the deck ([`DECK_FILE`]) and, if the models need a compatibility
/// mode, a `.spiceinit` setting it (ngspice reads it from its working directory before the deck;
/// the mode cannot be set from the deck).
pub(crate) fn prepare_run_dir(deck: &SpiceDeck) -> io::Result<RunDir> {
    let dir = RunDir::create()?;
    fs::write(dir.join(DECK_FILE), deck.to_string())?;
    if let Some(mode) = deck.netlist().ngbehavior() {
        fs::write(dir.join(".spiceinit"), format!("set ngbehavior={mode}\n"))?;
    }
    Ok(dir)
}

/// The result of a run that wrote `log`: an error if it did not `succeed` or the log reports one.
pub(crate) fn finish_run(
    dir: RunDir,
    log: String,
    succeeded: bool,
) -> Result<Simulation, SimulationError> {
    if !succeeded || log.lines().any(is_error_line) {
        // The directory is kept for inspection.
        return Err(SimulationError::Ngspice {
            log,
            dir: dir.keep(),
        });
    }
    Ok(Simulation { dir, log })
}

/// Whether ngspice reports an error with `line`: it starts with the word `error` (`Error: ...`,
/// `ERROR ...`), but is not a value printed as `name = value` (`error_max = 1.0` of `meas`).
fn is_error_line(line: &str) -> bool {
    let line = line.trim_start();
    let end = line
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(line.len());
    let (word, rest) = line.split_at(end);
    word.eq_ignore_ascii_case("error") && !rest.trim_start().starts_with('=')
}

/// A temporary directory a deck runs in, removed when dropped unless kept.
#[derive(Debug)]
pub(crate) struct RunDir {
    /// `None` once kept.
    path: Option<PathBuf>,
}

impl RunDir {
    /// A new, empty directory in the temporary directory of the system.
    fn create() -> io::Result<Self> {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        loop {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "{}-sim-{}-{n}",
                env!("CARGO_PKG_NAME"),
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(RunDir { path: Some(path) }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        self.path.as_deref().expect("the directory is not kept yet")
    }

    /// The path of the directory, which is no longer removed.
    fn keep(mut self) -> PathBuf {
        self.path.take().expect("the directory is not kept yet")
    }
}

impl std::ops::Deref for RunDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        self.path()
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_dir_all(path);
        }
    }
}

/// The results of a successful run. The run directory is removed when this is dropped.
#[derive(Debug)]
pub struct Simulation {
    dir: RunDir,
    log: String,
}

impl Simulation {
    /// The output of ngspice.
    pub fn log(&self) -> &str {
        &self.log
    }

    /// The directory the deck ran in.
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    /// Reads a file written by `wrdata` in the control block.
    ///
    /// # Errors
    /// [`SimulationError::Io`] if the file cannot be read, and [`SimulationError::Table`] if it
    /// is not a `wrdata` table.
    pub fn table(&self, file: &str) -> Result<Table, SimulationError> {
        let text = fs::read_to_string(self.dir.join(file))?;
        Table::parse(&text).map_err(|reason| SimulationError::Table {
            file: file.to_string(),
            reason,
        })
    }

    /// A value printed as `name = value` (by `meas` or `print`), matched case-insensitively.
    pub fn value(&self, name: &str) -> Option<f64> {
        self.log.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            if !key.trim().eq_ignore_ascii_case(name) {
                return None;
            }
            value.split_whitespace().next()?.parse().ok()
        })
    }

    /// [`Self::value`] for tests.
    ///
    /// # Panics
    /// If the value is not found, with the log of ngspice in the message.
    #[track_caller]
    pub fn expect_value(&self, name: &str) -> f64 {
        self.value(name)
            .unwrap_or_else(|| panic!("{name} not found in the output of ngspice:\n{}", self.log))
    }
}

/// Columns of a `wrdata` file written with `wr_vecnames` and `wr_singlescale`:
/// the scale (time, frequency, ...) followed by one column per real vector.
/// Write complex AC vectors through `vdb()`, `vm()`, `vp()`, `real()` or `imag()`.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    names: Vec<String>,
    columns: Vec<Vec<f64>>,
}

impl Table {
    fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text.lines().filter(|line| !line.trim().is_empty());
        let names: Vec<String> = lines
            .next()
            .ok_or("the file is empty")?
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let mut columns = vec![Vec::new(); names.len()];
        for (i, line) in lines.enumerate() {
            let values: Vec<f64> = line
                .split_whitespace()
                .map(|value| {
                    value
                        .parse()
                        .map_err(|_| format!("row {}: {value:?} is not a number", i + 1))
                })
                .collect::<Result<_, _>>()?;
            if values.len() != names.len() {
                return Err(format!(
                    "row {} has {} values for {} names (complex vectors are written as two columns)",
                    i + 1,
                    values.len(),
                    names.len()
                ));
            }
            for (column, value) in columns.iter_mut().zip(values) {
                column.push(value);
            }
        }
        Ok(Table { names, columns })
    }

    /// Vector names as written in the header (e.g. `frequency`, `vdb("/out")`).
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// The scale (first column).
    pub fn scale(&self) -> &[f64] {
        &self.columns[0]
    }

    /// A column by its name in the header, matched case-insensitively and ignoring double quotes:
    /// ngspice writes `v("/out")` as `v(/out)`, but keeps the quotes of `vdb("/out")`.
    pub fn column(&self, name: &str) -> Option<&[f64]> {
        let normalize = |s: &str| s.replace('"', "").to_ascii_lowercase();
        let name = normalize(name);
        let index = self.names.iter().position(|n| normalize(n) == name)?;
        Some(&self.columns[index])
    }

    /// The value of a column at `x` on the scale, interpolated linearly
    /// (`None` outside the scale or for an unknown column).
    pub fn interpolate(&self, name: &str, x: f64) -> Option<f64> {
        let ys = self.column(name)?;
        let xs = self.scale();
        let i = xs
            .windows(2)
            .position(|w| (w[0] <= x && x <= w[1]) || (w[1] <= x && x <= w[0]))?;
        let (x0, x1, y0, y1) = (xs[i], xs[i + 1], ys[i], ys[i + 1]);
        Some(if x1 == x0 {
            y0
        } else {
            y0 + (y1 - y0) * (x - x0) / (x1 - x0)
        })
    }
}

/// An error running a deck or reading its results.
#[derive(Debug)]
#[non_exhaustive]
pub enum SimulationError {
    /// ngspice cannot be run. The run directory, if any, is removed.
    Io(io::Error),
    /// ngspice failed or printed an error. The run directory is kept.
    Ngspice {
        /// The output of ngspice.
        log: String,
        /// The run directory, with the deck (`deck.cir`) to reproduce the run.
        dir: PathBuf,
    },
    /// A `wrdata` file cannot be read as a table.
    Table {
        /// The file name passed to [`Simulation::table`].
        file: String,
        /// Why it cannot be read.
        reason: String,
    },
    /// The ngspice shared library cannot be loaded or used.
    SharedLibrary(String),
    /// The models of a co-simulation do not match the devices of the deck.
    Cosimulation(String),
}

impl fmt::Display for SimulationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SimulationError::Io(e) => write!(f, "cannot run ngspice: {e}"),
            SimulationError::Ngspice { log, dir } => {
                write!(
                    f,
                    "ngspice reported an error (run directory: {})\n{log}",
                    dir.display()
                )
            }
            SimulationError::Table { file, reason } => write!(f, "cannot read {file}: {reason}"),
            SimulationError::SharedLibrary(reason) => {
                write!(f, "cannot use the ngspice shared library: {reason}")
            }
            SimulationError::Cosimulation(reason) => write!(f, "cannot co-simulate: {reason}"),
        }
    }
}

impl std::error::Error for SimulationError {}

impl From<io::Error> for SimulationError {
    fn from(e: io::Error) -> Self {
        SimulationError::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simulation(log: &str) -> Simulation {
        Simulation {
            dir: RunDir {
                path: Some(std::env::temp_dir().join("cuprite_wire-no-such-run-dir")),
            },
            log: log.to_string(),
        }
    }

    #[test]
    fn test_value() {
        let sim = simulation("Note: ...\nf3db                =  1.591549e+03\n");
        assert_eq!(sim.value("F3DB"), Some(1591.549));
        assert_eq!(sim.expect_value("f3db"), 1591.549);
        assert_eq!(sim.value("gain"), None);
    }

    #[test]
    #[should_panic(expected = "gain not found in the output of ngspice:\nNote: ...")]
    fn test_expect_value_panics_with_the_log() {
        simulation("Note: ...\n").expect_value("gain");
    }

    /// The run directory is removed when dropped, and kept when ngspice reports an error.
    #[test]
    fn test_run_dir_removed() {
        let dir = RunDir::create().unwrap();
        let path = dir.to_path_buf();
        assert!(path.is_dir());
        drop(dir);
        assert!(!path.exists());

        let dir = RunDir::create().unwrap();
        let path = dir.to_path_buf();
        let error = finish_run(dir, "Error: no circuit loaded.".into(), true).unwrap_err();
        assert!(matches!(&error, SimulationError::Ngspice { dir, .. } if *dir == path));
        assert!(path.is_dir());
        fs::remove_dir(&path).unwrap();
    }

    #[test]
    fn test_is_error_line() {
        assert!(is_error_line("Error: unknown subckt: x1 out 0 nosuch"));
        assert!(is_error_line("  ERROR on line 3"));
        assert!(is_error_line("error"));
        assert!(!is_error_line(
            "error_max           =  1.000000e+00 at=  5.000000e-03"
        ));
        assert!(!is_error_line("error               =  1.000000e+00"));
        assert!(!is_error_line("errors: none"));
        assert!(!is_error_line("PPerror: syntax error in line segment"));
        assert!(!is_error_line("Warning: error ignored"));
    }

    #[test]
    fn test_parse_table() {
        let text = " frequency       vdb(\"/out\")    \n 1.0e+01 -1.5e-02 \n 1.0e+02 -3.0e+00 \n";
        let table = Table::parse(text).unwrap();
        assert_eq!(table.names(), ["frequency", "vdb(\"/out\")"]);
        assert_eq!(table.scale(), [10.0, 100.0]);
        assert_eq!(table.column("VDB(\"/out\")"), Some(&[-0.015, -3.0][..]));
        assert_eq!(table.column("vdb(/out)"), table.column("vdb(\"/out\")"));
        assert_eq!(
            table.interpolate("vdb(\"/out\")", 55.0),
            Some(-0.015 + (-3.0 + 0.015) * 0.5)
        );
        assert_eq!(table.interpolate("vdb(\"/out\")", 1000.0), None);
        assert!(Table::parse("x y\n1 2 3\n").is_err());
        assert!(Table::parse("").is_err());
    }
}
