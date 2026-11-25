//! The interface shared by the signal stages (the preamp, the filter).

use cuprite_wire::{circuit::Node, subcircuit::Interface};

/// The interface of a signal stage: the signal in and out, the reference it sits at, and the
/// supply.
#[derive(Interface)]
pub struct StagePorts<T = Node> {
    pub input: T,
    pub vref: T,
    pub vcc: T,
    pub gnd: T,
    pub output: T,
}
