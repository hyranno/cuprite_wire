//! The SPICE model of a part given by the `sim` block.

use std::collections::BTreeMap;

use super::{PartError, PinDef, model_error};
use crate::{
    binding::{self, BindOptions, ModelBinding, PartPin, PartPins},
    resolve::Resolver,
    sim::{self, Elements, ModelMode},
    spice_lib::spice_path,
};

/// Binds the models of the `sim` block, with pad numbers.
pub(super) fn sim_block(
    bindings: &[ModelBinding],
    mode: ModelMode,
    pins: &[PinDef],
    internal_connections: &[Vec<String>],
    resolver: &mut Resolver,
) -> Result<Elements, PartError> {
    // The pins in pad numbers, connected as the part connects them.
    let pad_of: BTreeMap<&str, &str> = pins
        .iter()
        .map(|pin| (pin.id.as_str(), pin.pad.as_str()))
        .collect();
    let groups: Vec<Vec<String>> = internal_connections
        .iter()
        .map(|group| {
            group
                .iter()
                .map(|id| pad_of[id.as_str()].to_string())
                .collect()
        })
        .collect();
    // Duplicate pins are one pin of the model: the first, to which the others are connected.
    let part_pins: Vec<PartPin> = crate::unique_by(pins, |pin| pin.pad.as_str())
        .map(|pin| PartPin {
            id: pin.pad.clone(),
            name: pin.name.clone(),
        })
        .collect();
    let options = BindOptions {
        pins: PartPins::Given(&part_pins),
        strict: true,
        internal_connections: &groups,
        mode,
    };
    binding::bind(bindings, &options, resolver).map_err(model_error)
}

/// The `Sim.*` fields KiCad would have for a model of the `sim` block (with a single model), so that
/// the netlist carries the mapping as KiCad writes it.
pub(super) fn sim_fields(
    bindings: &[ModelBinding],
    elements: &[sim::ElementDef],
) -> Vec<(String, String)> {
    let ([binding], [element]) = (bindings, elements) else {
        return Vec::new();
    };
    let Some(info) = &element.info else {
        return Vec::new();
    };
    let mut fields = vec![
        ("Sim.Device".to_string(), info.sim_device.to_string()),
        ("Sim.Library".to_string(), spice_path(&info.path)),
        ("Sim.Name".to_string(), info.name.clone()),
        (
            "Sim.Pins".to_string(),
            sim::assignments(element.nodes.iter().zip(&info.ports)),
        ),
    ];
    if !binding.params.is_empty() {
        fields.push((
            "Sim.Params".to_string(),
            sim::assignments(binding.params.iter().map(|(k, v)| (k, v))),
        ));
    }
    fields
}
