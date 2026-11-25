//! The pins of a part: their IDs, the pads they map to and the connections inside the part.

use std::collections::{BTreeMap, BTreeSet};

use super::{OptionKey, PartError, PinDef, err};
use crate::{
    sim::{self, Elements, SpiceDef},
    symbol::{ElectricalType, Symbol},
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
    Ok(())
}

/// The associated constants the macros generate besides the pin constants: `PINS`, and for an
/// IBIS part the constants of the IBIS file and pin.
fn reserved_constants(spice: &SpiceDef) -> &'static [&'static str] {
    match spice {
        SpiceDef::Ibis(_) => &["PINS", "LIBRARY", "COMPONENT", "IBIS_PIN"],
        _ => &["PINS"],
    }
}

/// The constants of the pins given an ID that can be one (`out_a` -> `OUT_A`), as
/// `(constant name, pin ID)`, checked against each other and the other constants of the part.
pub(super) fn pin_constants(
    renames: &[(String, String)],
    spice: &SpiceDef,
) -> Result<Vec<(String, String)>, PartError> {
    let reserved = reserved_constants(spice);
    let mut constants: BTreeMap<String, &str> = BTreeMap::new();
    let mut result = Vec::new();
    for (_, id) in renames {
        if !is_identifier(id) {
            continue;
        }
        let name = id.to_ascii_uppercase();
        if reserved.contains(&name.as_str()) {
            return err(
                OptionKey::Pins,
                format!("pin ID {id:?} would make the constant {name}, which the part already has"),
            );
        }
        if let Some(other) = constants.insert(name.clone(), id) {
            return err(
                OptionKey::Pins,
                format!("pin IDs {other:?} and {id:?} differ only in case"),
            );
        }
        result.push((name, id.clone()));
    }
    Ok(result)
}

/// An ID that is an identifier (not `_`) in upper case. Upper-case words are never keywords.
fn is_identifier(text: &str) -> bool {
    text.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && text != "_"
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

/// The pins of a part made from a symbol: a pin per pad of the symbol pins, then the extra pads;
/// IDs default to the pad numbers (see [`default_pin_ids`] for duplicate pins), renamed by
/// `renames`.
pub(super) fn symbol_part_pins(
    symbol: &Symbol,
    extra_pads: &[String],
    renames: &[(String, String)],
) -> Result<Vec<PinDef>, PartError> {
    let mut pins: Vec<PinDef> = symbol
        .pins
        .iter()
        .flat_map(|pin| {
            pin.pads.iter().map(|pad| PinDef {
                id: pad.clone(),
                pad: pad.clone(),
                name: pin.name.clone(),
                electrical_type: pin.electrical_type,
                unit: pin.unit,
            })
        })
        .collect();
    pins.extend(extra_pads.iter().map(|pad| PinDef {
        id: pad.clone(),
        pad: pad.clone(),
        name: None,
        electrical_type: ElectricalType::Passive,
        unit: 0,
    }));
    let default_ids = default_pin_ids(&pins);
    for (pin, id) in pins.iter_mut().zip(&default_ids) {
        pin.id.clone_from(id);
    }
    rename_pins(&mut pins, renames, PinKeys::PadNumbers)?;
    Ok(pins)
}

/// The pins of [`symbol_part_pins`] connected to global nets, as `(pin ID, net name)`: those of
/// hidden `power_in` symbol pins.
pub(super) fn global_labels(symbol: &Symbol, pins: &[PinDef]) -> Vec<(String, String)> {
    // The global net of each pin of the symbol pins, in the same order.
    let global_nets = symbol.pins.iter().flat_map(|pin| {
        let net = (pin.hidden && pin.electrical_type == ElectricalType::PowerIn)
            .then(|| pin.name.clone())
            .flatten();
        pin.pads.iter().map(move |_| net.clone())
    });
    pins.iter()
        .zip(global_nets)
        .filter_map(|(pin, net)| Some((pin.id.clone(), net?)))
        .collect()
}

/// The internal connections of a part made from a symbol: the pads of a stacked pin notation,
/// pins at the same position, the duplicate pins of each pad, then the `given` ones.
pub(super) fn symbol_internal_connections(
    symbol: &Symbol,
    pins: &[PinDef],
    ids: &PinIds,
    given: &[Vec<String>],
) -> Result<Vec<Vec<String>>, PartError> {
    let mut connections: Vec<Vec<String>> = symbol
        .pins
        .iter()
        .filter(|pin| pin.pads.len() > 1)
        .map(|pin| ids.of(&pin.pads))
        .collect();
    connections.extend(symbol.stacked_pins().iter().map(|group| ids.of_pins(group)));
    let mut pins_of_pad: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for pin in pins {
        pins_of_pad
            .entry(pin.pad.as_str())
            .or_default()
            .push(pin.id.clone());
    }
    connections.extend(pins_of_pad.into_values().filter(|ids| ids.len() > 1));
    connections.extend(check_internal_connections(given, pins)?);
    Ok(connections)
}
