//! Helpers shared by the integration tests. Each test crate uses only some of them.
#![allow(dead_code, unused_imports)]

mod golden;

pub use golden::check_golden;

use cuprite_wire::{
    circuit::CircuitBuilder,
    spice::{SimulationError, SpiceDeck},
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
