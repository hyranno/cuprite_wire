//! SPICE-specific part information, netlist export and simulation with ngspice.
//!
//! See the guide [Exporting a SPICE netlist and simulating](https://github.com/hyranno/cuprite_wire/blob/main/doc/spice_netlist_usage.md).

pub mod deck;
pub mod netlist;
pub mod part;
pub mod simulation;
pub mod sources;

pub use deck::SpiceDeck;
pub use netlist::{GROUND, SpiceNetlist, spice_node_name};
pub use part::{ModelSource, SpiceDialect, SpiceElement, SpicePartInfo, SpiceProblem};
pub use simulation::{Ngspice, Simulation, SimulationError, Table};
pub use sources::{Ammeter, IndependentSource, SourceKind, Waveform};

#[cfg(test)]
mod tests;
