//! A SPICE deck: a netlist with the analyses of a test bench.

use std::{borrow::Cow, fmt};

#[cfg(feature = "shared-ngspice")]
use std::path::Path;

use crate::spice::{Ngspice, Simulation, SimulationError, SpiceNetlist};

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
}

impl SpiceDeck {
    pub fn new(netlist: SpiceNetlist) -> Self {
        SpiceDeck {
            netlist,
            directives: Vec::new(),
            control: Vec::new(),
        }
    }

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

    fn has_control(&self) -> bool {
        !self.control.is_empty()
    }

    /// Runs the deck with `ngspice` from `PATH` ([`Ngspice::default`]).
    pub fn run(&self) -> Result<Simulation, SimulationError> {
        Ngspice::default().run(self)
    }
}

#[cfg(feature = "shared-ngspice")]
/// Front-end commands whose first argument is an output file.
const OUTPUT_COMMANDS: [&str; 3] = ["wrdata", "write", "wrs2p"];

impl SpiceDeck {
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
    if !OUTPUT_COMMANDS.iter().any(|c| c.eq_ignore_ascii_case(name))
        || file.is_empty()
        || Path::new(file).is_absolute()
    {
        return Cow::Borrowed(command);
    }
    Cow::Owned(format!("{name} {}{rest}", dir.join(file).display()))
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
        assert_eq!(output_in("write", dir), "write");
        assert_eq!(output_in("tran 1u 1m", dir), "tran 1u 1m");
    }
}
