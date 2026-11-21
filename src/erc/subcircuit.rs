use std::panic::Location;

use super::{ErcReport, ErcSettings};
use crate::{
    error::CircuitErrors,
    parts::PinType,
    subcircuit::{Interface, InterfaceWith, SubcircuitGenerator, construct},
};

/// Runs ERC on a subcircuit alone.
///
/// `ports` gives, for each node of the subcircuit's interface, the pin the circuit around it is
/// expected to connect, as a virtual pin named `port <name>`: e.g. [`PinType::Output`] for a node
/// driven from outside, [`PinType::Input`] for one read from outside, and [`PinType::PowerOut`] for
/// a supply. Nodes given `None` are checked as if nothing were connected to them.
///
/// # Errors
/// The errors of the netlist conversion ([`BuiltCircuit::to_netlist`](crate::circuit::BuiltCircuit::to_netlist))
/// if the subcircuit cannot be converted.
#[track_caller]
pub fn check_subcircuit<G: SubcircuitGenerator>(
    generator: &G,
    options: &G::Options,
    ports: &InterfaceWith<G, Option<PinType>>,
    settings: &ErcSettings,
) -> Result<ErcReport, CircuitErrors> {
    let (mut circuit, interface) = construct(generator, options, Location::caller());
    let names = <G::Interface as Interface>::names();
    for ((node, name), pin_type) in interface
        .values()
        .into_iter()
        .zip(names)
        .zip(ports.values())
    {
        if let Some(pin_type) = pin_type {
            circuit.virtual_pin(*node, *pin_type, &format!("port {name}"));
        }
    }
    Ok(circuit.build(options).to_netlist()?.erc(settings))
}
