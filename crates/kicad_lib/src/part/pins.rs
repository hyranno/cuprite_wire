//! The pins of a part: their IDs, the pads they map to and the connections inside the part.

use std::collections::{BTreeMap, BTreeSet};

use super::{OptionKey, PartError, PinDef, err};
use crate::{
    sim::{self, Elements, SpiceDef},
    symbol::Symbol,
};

/// Pin IDs by pad number (of the first pin of duplicate pins), and pads by symbol pin number.
pub(super) struct PinIds<'a> {
    ids: BTreeMap<&'a str, &'a str>,
    pads: BTreeMap<&'a str, &'a [String]>,
}

impl<'a> PinIds<'a> {
    pub(super) fn new(pins: &'a [PinDef], symbol: &'a Symbol) -> Self {
        PinIds {
            // The first pin of each pad (`rev` so that it is inserted last).
            ids: pins
                .iter()
                .rev()
                .map(|pin| (pin.pad.as_str(), pin.id.as_str()))
                .collect(),
            pads: symbol
                .pins
                .iter()
                .map(|pin| (pin.number.as_str(), pin.pads.as_slice()))
                .collect(),
        }
    }

    /// The IDs of pads.
    pub(super) fn of(&self, pads: &[String]) -> Vec<String> {
        pads.iter()
            .map(|n| self.ids[n.as_str()].to_string())
            .collect()
    }

    /// The IDs of symbol pins (of the first pad of a stacked pin notation).
    pub(super) fn of_pins(&self, numbers: &[String]) -> Vec<String> {
        self.of(&self.first_pads(numbers))
    }

    fn first_pads(&self, numbers: &[String]) -> Vec<String> {
        numbers
            .iter()
            .map(|n| self.pads[n.as_str()][0].clone())
            .collect()
    }

    /// All pads of symbol pins.
    pub(super) fn symbol_pin_pads(&self, numbers: &[String]) -> Vec<String> {
        numbers
            .iter()
            .flat_map(|n| self.pads[n.as_str()].iter().cloned())
            .collect()
    }

    /// Elements with symbol pin numbers turned into pads (the first of a stacked pin notation,
    /// whose pads are connected).
    pub(super) fn with_first_pads(
        &self,
        mut elements: Vec<sim::ElementDef>,
    ) -> Vec<sim::ElementDef> {
        for element in &mut elements {
            element.nodes = self.first_pads(&element.nodes);
        }
        elements
    }

    /// The SPICE description with pin IDs, from one with pad numbers.
    pub(super) fn elements(&self, mut elements: Elements) -> SpiceDef {
        for element in &mut elements.elements {
            element.nodes = self.of(&element.nodes);
        }
        elements.unused_pins = self.of(&elements.unused_pins);
        SpiceDef::Elements(elements)
    }
}

/// What the keys of `pins` are.
#[derive(Debug, Clone, Copy)]
pub(super) enum PinKeys<'a> {
    /// Pad numbers, or default pin IDs (`6#2`).
    PadNumbers,
    /// Port names of `model`, matched case-insensitively.
    PortNames { model: &'a str },
}

/// Applies `pins` (pin ID by pad number or port name) and checks that the IDs are unique.
pub(super) fn rename_pins(
    pins: &mut [PinDef],
    renames: &[(String, String)],
    keys: PinKeys,
) -> Result<(), PartError> {
    let what = match keys {
        PinKeys::PadNumbers => "pin",
        PinKeys::PortNames { .. } => "port",
    };
    let default_ids: Vec<String> = pins.iter().map(|pin| pin.id.clone()).collect();
    let mut renamed = BTreeSet::new();
    for (key, id) in renames {
        let normalized = match keys {
            PinKeys::PadNumbers => key.clone(),
            PinKeys::PortNames { .. } => key.to_ascii_lowercase(),
        };
        if !renamed.insert(normalized) {
            return err(OptionKey::Pins, format!("{what} {key:?} is renamed twice"));
        }
        if id.is_empty() {
            return err(OptionKey::Pins, format!("empty pin ID for {what} {key:?}"));
        }
        // A pad number stands for its first pin; the other duplicate pins are given by their
        // default IDs (`6#2`).
        let index = match keys {
            PinKeys::PadNumbers => pins
                .iter()
                .position(|pin| &pin.pad == key)
                .or_else(|| default_ids.iter().position(|id| id == key)),
            PinKeys::PortNames { .. } => pins
                .iter()
                .position(|pin| pin.pad.eq_ignore_ascii_case(key)),
        };
        let pin = index.map(|i| &mut pins[i]);
        match pin {
            Some(pin) => pin.id = id.clone(),
            None => {
                let known: Vec<&str> = pins.iter().map(|pin| pin.pad.as_str()).collect();
                return err(
                    OptionKey::Pins,
                    match keys {
                        PinKeys::PadNumbers => format!("the part has no pin {key:?}"),
                        PinKeys::PortNames { model } => format!(
                            "the model {model} has no port {key:?}; its ports are {}",
                            known.join(" ")
                        ),
                    },
                );
            }
        }
    }
    let mut ids = BTreeSet::new();
    for pin in pins.iter() {
        if !ids.insert(pin.id.as_str()) {
            return err(
                OptionKey::Pins,
                format!("pin ID {:?} is used twice", pin.id),
            );
        }
    }
    // The macros make an upper-case constant of each given ID (`out` -> `OUT`).
    let mut upper: BTreeMap<String, &str> = BTreeMap::new();
    for (_, id) in renames {
        if let Some(other) = upper.insert(id.to_ascii_uppercase(), id)
            && other != id
        {
            return err(
                OptionKey::Pins,
                format!("pin IDs {other:?} and {id:?} differ only in case"),
            );
        }
    }
    Ok(())
}

/// The default pin IDs: the pad numbers, with `#2`, `#3`, ... appended for the second and later
/// duplicate pins of a pad (the first is the one KiCad writes to the netlist).
pub(super) fn default_pin_ids(pins: &[PinDef]) -> Vec<String> {
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    pins.iter()
        .map(|pin| {
            let n = count.entry(pin.pad.as_str()).or_default();
            *n += 1;
            if *n == 1 {
                pin.pad.clone()
            } else {
                format!("{}#{n}", pin.pad)
            }
        })
        .collect()
}

pub(super) fn check_internal_connections(
    groups: &[Vec<String>],
    pins: &[PinDef],
) -> Result<Vec<Vec<String>>, PartError> {
    for group in groups {
        if group.len() < 2 {
            return err(
                OptionKey::InternalConnections,
                "a group needs at least two pins",
            );
        }
        if let Some(unknown) = group
            .iter()
            .find(|id| !pins.iter().any(|pin| &pin.id == *id))
        {
            return err(
                OptionKey::InternalConnections,
                format!("the part has no pin ID {unknown:?}"),
            );
        }
    }
    Ok(groups.to_vec())
}
