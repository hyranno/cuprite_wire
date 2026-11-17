//! A SPICE deck: a netlist with the analyses of a test bench.

use std::fmt;

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

    pub(crate) fn has_control(&self) -> bool {
        !self.control.is_empty()
    }

    /// Runs the deck with the simulator selected by the environment ([`Ngspice::from_env`]).
    pub fn run(&self) -> Result<Simulation, SimulationError> {
        Ngspice::from_env().run(self)
    }
}

impl fmt::Display for SpiceDeck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.netlist.fmt_body(f)?;
        for directive in &self.directives {
            writeln!(f, "{directive}")?;
        }
        if self.has_control() {
            writeln!(f, ".control")?;
            for command in CONTROL_PREAMBLE
                .iter()
                .copied()
                .chain(self.control.iter().map(String::as_str))
            {
                writeln!(f, "{command}")?;
            }
            writeln!(f, "quit")?;
            writeln!(f, ".endc")?;
        }
        writeln!(f, ".end")
    }
}
