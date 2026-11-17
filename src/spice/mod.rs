//! SPICE-specific part information, netlist export and simulation with ngspice.

pub mod deck;
pub mod netlist;
pub mod part;
pub mod simulation;
pub mod sources;

pub use deck::SpiceDeck;
pub use netlist::{GROUND, SpiceNetlist, spice_node_name};
pub use part::{ModelSource, SpiceElement, SpicePartInfo, SpiceProblem};
pub use simulation::{NGSPICE_ENV, Ngspice, Simulation, SimulationError, Table};
pub use sources::{Ammeter, IndependentSource, SourceKind, Waveform};

#[cfg(test)]
mod tests;
