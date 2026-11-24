//! A SPICE deck: a netlist with the analyses of a test bench.

use std::{borrow::Cow, fmt};

#[cfg(feature = "shared-ngspice")]
use std::path::Path;

#[cfg(feature = "shared-ngspice")]
use crate::spice::spice_path;

use crate::spice::{
    Ngspice, Simulation, SimulationError, SpiceNetlist,
    analysis::{Analysis, AnalysisCommand, wrdata_command},
};

/// Front-end settings written at the start of the control block, so that `wrdata` writes
/// a header line and one shared scale column ([`Simulation::table`] reads that form).
const CONTROL_PREAMBLE: [&str; 3] = [
    "set filetype=ascii",
    "set wr_singlescale",
    "set wr_vecnames",
];

/// A [`SpiceNetlist`] with dot directives (`.options`, `.tran ...`) and a `.control` block.
/// Analyses belong to the test, not to the circuit, so the same netlist can be simulated
/// in different ways.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpiceDeck {
    netlist: SpiceNetlist,
    directives: Vec<String>,
    control: Vec<String>,
    /// The identity of each typed analysis with the file `wrdata` writes its probes into
    /// (`None` without probes).
    analyses: Vec<(u64, Option<String>)>,
}

impl SpiceDeck {
    /// A deck of `netlist` without directives or control commands.
    pub fn new(netlist: SpiceNetlist) -> Self {
        SpiceDeck {
            netlist,
            directives: Vec::new(),
            control: Vec::new(),
            analyses: Vec::new(),
        }
    }

    /// The netlist of the circuit.
    pub fn netlist(&self) -> &SpiceNetlist {
        &self.netlist
    }

    /// Adds a line after the elements, e.g. `.options reltol=1e-4`, `.ic v(/out)=0` or `.tran 1u 1m`.
    pub fn directive(mut self, line: impl Into<String>) -> Self {
        self.directives.push(line.into());
        self
    }

    /// Adds ngspice front-end commands to the `.control` block, e.g. `ac dec 10 1 1Meg`,
    /// `wrdata ac.txt vdb("/out")` or `meas ac f3db WHEN vdb("/out")=-3`.
    ///
    /// The block starts with `set filetype=ascii`, `set wr_singlescale` and `set wr_vecnames`,
    /// and ends with `quit`. Quote node names containing `/` or `-` in expressions: `v("/out")`.
    pub fn control<S: Into<String>>(mut self, commands: impl IntoIterator<Item = S>) -> Self {
        self.control.extend(commands.into_iter().map(Into::into));
        self
    }

    /// Adds `analysis` to the `.control` block, after the commands added before it, with a `wrdata`
    /// command recording its probes. Read the result with [`Simulation::result`].
    ///
    /// # Panics
    /// If the deck already has the analysis.
    pub fn analysis(mut self, analysis: &impl Analysis) -> Self {
        assert!(
            self.analysis_file(analysis).is_none(),
            "the deck already has the analysis {}",
            analysis.command()
        );
        let file = format!("analysis{}.txt", self.analyses.len() + 1);
        self.control.push(analysis.command());
        let wrdata = wrdata_command(analysis, &file);
        let file = wrdata.map(|wrdata| {
            self.control.push(wrdata);
            file
        });
        self.analyses.push((analysis.id(), file));
        self
    }

    /// The file `wrdata` writes the probes of `analysis` into: `None` if the deck does not have the
    /// analysis, `Some(None)` if it has no probes.
    pub(crate) fn analysis_file(&self, analysis: &dyn AnalysisCommand) -> Option<Option<&str>> {
        self.analyses
            .iter()
            .find(|(id, _)| *id == analysis.id())
            .map(|(_, file)| file.as_deref())
    }

    /// The typed analyses with their output files, as [`Self::analysis_file`].
    pub(crate) fn analyses(&self) -> &[(u64, Option<String>)] {
        &self.analyses
    }

    pub(crate) fn has_control(&self) -> bool {
        !self.control.is_empty()
    }

    /// Runs the deck with `ngspice` from `PATH` ([`Ngspice::default`]).
    ///
    /// # Errors
    /// See [`Ngspice::run`].
    pub fn run(&self) -> Result<Simulation, SimulationError> {
        Ngspice::default().run(self)
    }
}

#[cfg(feature = "shared-ngspice")]
/// Front-end commands whose first argument is an output file.
const OUTPUT_COMMANDS: [&str; 3] = ["wrdata", "write", "wrs2p"];

#[cfg(feature = "shared-ngspice")]
/// The file `write` writes when it is given none.
const DEFAULT_RAW_FILE: &str = "rawspice.raw";

impl SpiceDeck {
    #[cfg(feature = "shared-ngspice")]
    /// The first control command that runs in the background thread of the shared library
    /// (`bg_run`, `bg_halt`, ...), which the runs of [`SharedNgspice`](crate::spice::SharedNgspice)
    /// do not wait for.
    pub(crate) fn background_command(&self) -> Option<&str> {
        self.control
            .iter()
            .flat_map(|commands| commands.lines())
            .filter_map(|line| line.split_whitespace().next())
            .find(|command| {
                command
                    .get(..3)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bg_"))
            })
    }

    #[cfg(feature = "shared-ngspice")]
    /// The deck for the ngspice shared library, which runs in the working directory of the
    /// process: relative output files of `wrdata`, `write` and `wrs2p` are put in `dir`, and the
    /// control block does not end with `quit` (which would make the library exit).
    pub(crate) fn for_shared_library(&self, dir: &Path) -> String {
        let mut text = String::new();
        self.write(&mut text, |command| output_in(command, dir), false)
            .expect("writing to a String does not fail");
        text
    }

    fn write<'a>(
        &'a self,
        f: &mut impl fmt::Write,
        command: impl Fn(&'a str) -> Cow<'a, str>,
        quit: bool,
    ) -> fmt::Result {
        self.netlist.fmt_body(f)?;
        for directive in &self.directives {
            writeln!(f, "{directive}")?;
        }
        if self.has_control() {
            writeln!(f, ".control")?;
            for line in CONTROL_PREAMBLE
                .iter()
                .copied()
                .chain(self.control.iter().map(String::as_str))
            {
                writeln!(f, "{}", command(line))?;
            }
            if quit {
                writeln!(f, "quit")?;
            }
            writeln!(f, ".endc")?;
        }
        writeln!(f, ".end")
    }
}

#[cfg(feature = "shared-ngspice")]
/// `command` with its output file put in `dir`, if it is an output command with a relative file.
fn output_in<'a>(command: &'a str, dir: &Path) -> Cow<'a, str> {
    let split = |s: &'a str| {
        let s = s.trim_start();
        let end = s.find(char::is_whitespace).unwrap_or(s.len());
        (&s[..end], &s[end..])
    };
    let (name, rest) = split(command);
    let (file, rest) = split(rest);
    if !OUTPUT_COMMANDS.iter().any(|c| c.eq_ignore_ascii_case(name)) {
        return Cow::Borrowed(command);
    }
    // `write` without a file writes the default raw file of ngspice.
    let file = match file {
        "" if name.eq_ignore_ascii_case("write") => DEFAULT_RAW_FILE,
        "" => return Cow::Borrowed(command),
        file if Path::new(file).is_absolute() => return Cow::Borrowed(command),
        file => file,
    };
    Cow::Owned(format!("{name} {}{rest}", spice_path(&dir.join(file))))
}

impl fmt::Display for SpiceDeck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write(f, Cow::Borrowed, true)
    }
}

#[cfg(all(test, feature = "shared-ngspice"))]
mod tests {
    use super::*;

    #[test]
    fn test_output_in() {
        let dir = Path::new("/run/dir");
        assert_eq!(
            output_in(r#"wrdata tran.txt v("/OUT")"#, dir),
            r#"wrdata /run/dir/tran.txt v("/OUT")"#
        );
        assert_eq!(output_in("  WRITE out.raw", dir), "WRITE /run/dir/out.raw");
        assert_eq!(
            output_in("wrdata /abs.txt v(1)", dir),
            "wrdata /abs.txt v(1)"
        );
        assert_eq!(output_in("write", dir), "write /run/dir/rawspice.raw");
        assert_eq!(output_in(" Write ", dir), "Write /run/dir/rawspice.raw");
        assert_eq!(output_in("wrs2p", dir), "wrs2p");
        assert_eq!(output_in("tran 1u 1m", dir), "tran 1u 1m");
    }
}
