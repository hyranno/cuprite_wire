//! SPICE-specific part information, netlist export and simulation with ngspice.
//!
//! See the guide [Exporting a SPICE netlist and simulating](https://github.com/hyranno/cuprite_wire/blob/main/doc/spice_netlist_usage.md).

pub mod analysis;
#[cfg(feature = "shared-ngspice")]
pub mod cosim;
pub mod deck;
pub mod digital;
#[cfg(feature = "ibis")]
pub mod ibis;
pub mod netlist;
pub mod part;
#[cfg(feature = "shared-ngspice")]
pub mod shared;
pub mod simulation;
pub mod sources;
mod subcircuit;
mod temp_dir;
pub mod value;
pub mod xspice;

pub use deck::SpiceDeck;
pub use netlist::{GROUND, SpiceNetlist, SpiceOptions, spice_node_name};
pub use part::{ModelSource, SpiceDialect, SpiceElement, SpicePartInfo, SpiceProblem};
#[cfg(feature = "shared-ngspice")]
pub use shared::SharedNgspice;
pub use simulation::{Ngspice, Simulation, SimulationError, Simulator, Table};
pub use sources::{Ammeter, IndependentSource, SourceKind, TransmissionLine, Waveform};
pub use value::{spice_number, spice_value};

#[cfg(test)]
mod tests;

pub(crate) use cuprite_wire_kicad_lib::spice_lib::spice_path;
