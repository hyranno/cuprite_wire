//! Helpers shared by the integration tests and the unit tests (`src/test_parts.rs`). Each test
//! crate uses only some of them.
#![allow(dead_code, unused_imports)]

mod golden;

pub use golden::check_golden;

use cuprite_wire::{
    circuit::CircuitBuilder,
    spice::{
        SimulationError, SpiceDeck,
        analysis::{Probe, SweepResult, Tran},
    },
};

pub fn strings(s: &[&str]) -> Vec<String> {
    s.iter().map(|s| s.to_string()).collect()
}

/// The SPICE deck of `circuit`, without front-end commands.
pub fn deck_of(circuit: &CircuitBuilder) -> SpiceDeck {
    circuit
        .build(&())
        .to_netlist()
        .unwrap()
        .to_spice()
        .unwrap()
        .deck()
}

/// The message of a failed simulation; removes the run directory that ngspice errors keep.
pub fn simulation_error_message(error: SimulationError) -> String {
    if let SimulationError::Ngspice { dir, .. } = &error {
        let _ = std::fs::remove_dir_all(dir);
    }
    error.to_string()
}

/// A transient analysis recording the voltage of the line `/LINE` (a local label of the circuit).
pub struct LineDeck {
    /// The deck with the analysis; more commands can be added to it.
    pub deck: SpiceDeck,
    tran: Tran,
    line: Probe,
}

impl LineDeck {
    /// The transient analysis of `circuit` with `step` until `stop`.
    pub fn new(circuit: &CircuitBuilder, step: f64, stop: f64) -> Self {
        let deck = deck_of(circuit);
        let line = deck.voltage("/LINE").unwrap();
        let tran = Tran::new(step, stop).probe(&line);
        LineDeck {
            deck: deck.analysis(&tran),
            tran,
            line,
        }
    }

    /// The voltage of the line in a run of [`Self::deck`].
    pub fn read(&self, simulation: &cuprite_wire::spice::Simulation) -> Line {
        Line {
            result: simulation.result(&self.tran).unwrap(),
            line: self.line.clone(),
        }
    }
}

/// The voltage of the line of a [`LineDeck`] over the time.
pub struct Line {
    result: SweepResult,
    line: Probe,
}

impl Line {
    /// The time points.
    pub fn time(&self) -> &[f64] {
        self.result.scale()
    }

    /// The voltage at each time point.
    pub fn volts(&self) -> &[f64] {
        self.result.get(&self.line).unwrap()
    }

    /// The voltage at `time`, interpolated.
    pub fn at(&self, time: f64) -> f64 {
        self.result.interpolate(&self.line, time).unwrap().unwrap()
    }
}
