use petgraph::graph::NodeIndex;

use super::{ErcReport, ErcSettings};
use crate::{error::CircuitErrors, parts::PinType, subcircuit::SubcircuitGenerator};

/// Runs ERC on a subcircuit alone.
///
/// `ports` gives, for interface nodes of the subcircuit, the pin the circuit around it is expected
/// to connect, as a virtual pin named `port <node index>`: e.g. [`PinType::Output`] for a node driven
/// from outside, [`PinType::Input`] for one read from outside, and [`PinType::PowerOut`] for a supply.
/// Interface nodes not in `ports` are checked as if nothing were connected to them.
///
/// Fails if the subcircuit cannot be converted to a netlist.
#[track_caller]
pub fn check_subcircuit<G: SubcircuitGenerator>(
    generator: &G,
    options: &G::Options,
    ports: &[(NodeIndex, PinType)],
    settings: &ErcSettings,
) -> Result<ErcReport, CircuitErrors> {
    let mut circuit = generator.construct_subcircuit(options);
    for &(node, pin_type) in ports {
        circuit.virtual_pin(node, pin_type, &format!("port {}", node.index()));
    }
    Ok(circuit.build(options).to_netlist()?.erc(settings))
}
