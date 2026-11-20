//! SPICE-specific part information, netlist export and simulation with ngspice.
//!
//! See the guide [Exporting a SPICE netlist and simulating](https://github.com/hyranno/cuprite_wire/blob/main/doc/spice_netlist_usage.md).

#[cfg(feature = "shared-ngspice")]
pub mod cosim;
pub mod deck;
pub mod digital;
pub mod netlist;
pub mod part;
#[cfg(feature = "shared-ngspice")]
pub mod shared;
pub mod simulation;
pub mod sources;
pub mod xspice;

pub use deck::SpiceDeck;
pub use netlist::{GROUND, SpiceNetlist, spice_node_name};
pub use part::{ModelSource, SpiceDialect, SpiceElement, SpicePartInfo, SpiceProblem};
#[cfg(feature = "shared-ngspice")]
pub use shared::SharedNgspice;
pub use simulation::{Ngspice, Simulation, SimulationError, Simulator, Table};
pub use sources::{Ammeter, IndependentSource, SourceKind, Waveform};

#[cfg(test)]
mod tests;
