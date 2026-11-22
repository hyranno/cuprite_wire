//! Turning a symbol (and a footprint), or a SPICE model, into the description of a part, as the
//! macros generate it.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Error,
    binding::{self, BindOptions, BindingField, ModelBinding, PartPin, PartPins},
    naming::LibId,
    resolve::Resolver,
    sim::{self, ModelMode, SimInput, SpiceDef, Value},
    symbol::{ElectricalType, Symbol},
    text_var,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FootprintOption {
    /// The symbol's `Footprint` property.
    #[default]
    FromSymbol,
    Given(String),
    /// No footprint (simulation-only parts); exporting the part to KiCad is an error.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpiceOption {
    /// From the `Sim.*` fields (or the `sim` block); an error if they cannot be converted.
    #[default]
    Auto,
    /// Not simulated: `spice()` returns `None`.
    None,
    /// Excluded from the simulation.
    Excluded,
}

/// The options shared by `kicad_part!` and `spice_part!`.
#[derive(Debug, Clone, Default)]
pub struct CommonOptions {
    /// `None`: the symbol's `Value`, or the model name.
    pub value: Option<Value>,
    pub ref_base: Option<String>,
    /// Pin ID by pin (or extra pad) number, or by port name.
    pub pins: Vec<(String, String)>,
    /// Groups of pin IDs connected inside the part, in addition to stacked pins.
    pub internal_connections: Vec<Vec<String>>,
    pub sim_model: ModelMode,
    /// Name of the generated type, used to name built-in SPICE models.
    pub type_name: String,
}

/// The options of `kicad_part!`.
#[derive(Debug, Clone, Default)]
pub struct PartOptions {
    pub common: CommonOptions,
    /// `Library:Name`.
    pub symbol: String,
    pub footprint: FootprintOption,
    /// Footprint pads without a symbol pin that become pins of the part.
    pub extra_pads: Vec<String>,
    pub spice: SpiceOption,
    pub fields: Vec<(String, String)>,
    /// Library models replacing the symbol's `Sim.*` fields (the `sim` block), with pin numbers.
    pub sim: Vec<ModelBinding>,
}

/// The options of `spice_part!`: the model (its `pins` are not used) and the common options,
/// whose `pins` rename ports.
#[derive(Debug, Clone, Default)]
pub struct SpicePartOptions {
    pub common: CommonOptions,
    pub model: ModelBinding,
}

/// The option an error is about, so that the macro can point at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKey {
    Symbol,
    Footprint,
    Value,
    RefBase,
    Pins,
    InternalConnections,
    ExtraPads,
    Spice,
    /// An option of a model: of `spice_part!`, or of the `element`-th model of the `sim` block.
    Model {
        element: usize,
        field: BindingField,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartError {
    pub key: OptionKey,
    pub error: Error,
}

fn err<T>(key: OptionKey, message: impl Into<String>) -> Result<T, PartError> {
    Err(PartError {
        key,
        error: Error::new(message),
    })
}

fn at(key: OptionKey) -> impl Fn(Error) -> PartError {
    move |error| PartError { key, error }
}

fn model_error(error: binding::BindingError) -> PartError {
    PartError {
        key: OptionKey::Model {
            element: error.element,
            field: error.field,
        },
        error: error.error,
    }
}

/// A pin of the generated part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinDef {
    pub id: String,
    /// Footprint pad number (the port name for parts made from a SPICE model).
    pub pad: String,
    pub name: Option<String>,
    pub electrical_type: ElectricalType,
    /// The symbol unit (0: shared, or an extra pad).
    pub unit: u32,
}

/// Everything the macros need to generate a part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartDef {
    /// `None` for parts made from a SPICE model, which cannot be exported to KiCad.
    pub kicad: Option<KicadDef>,
    pub ref_base: String,
    pub value: Value,
    pub pins: Vec<PinDef>,
    /// Groups of pin IDs.
    pub internal_connections: Vec<Vec<String>>,
    /// The SPICE description, with pin IDs.
    pub spice: SpiceDef,
}

/// What a part made from a KiCad symbol has for KiCad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KicadDef {
    pub symbol: LibId,
    pub footprint: Option<LibId>,
    pub fields: Vec<(String, String)>,
    pub exclude_from_bom: bool,
    pub exclude_from_board: bool,
    pub description: Option<String>,
    pub datasheet: Option<String>,
}

pub fn load_part(options: &PartOptions, resolver: &mut Resolver) -> Result<PartDef, PartError> {
    use OptionKey::*;
    let common = &options.common;
    let symbol_id = options
        .symbol
        .parse::<LibId>()
        .map_err(|e| at(Symbol)(e.into()))?;
    let lib = resolver
        .symbol_lib(symbol_id.library())
        .map_err(at(Symbol))?;
    let symbol = lib.symbol(symbol_id.name()).map_err(at(Symbol))?;
    check_symbol(&symbol).map_err(at(Symbol))?;

    let ref_base = match &common.ref_base {
        Some(ref_base) => ref_base.clone(),
        None => symbol
            .property("Reference")
            .unwrap_or_default()
            .trim_end_matches(|c: char| c == '?' || c.is_ascii_digit())
            .to_string(),
    };
    check_ref_base(&ref_base)?;
    let footprint = footprint(options, &symbol, &symbol_id, resolver)?;
    // The value with its text variables expanded from the fields (`fields` of the part).
    let value_with = |fields: &[(String, String)]| match &common.value {
        Some(sim::Value::Param) => Ok(sim::Value::Param),
        Some(sim::Value::Fixed(value)) => {
            expand_value(value, fields, &symbol, footprint.as_ref(), &options.sim)
                .map_err(at(Value))
        }
        None => expand_value(
            symbol.property("Value").unwrap_or_default(),
            fields,
            &symbol,
            footprint.as_ref(),
            &options.sim,
        )
        .map_err(at(Symbol)),
    };

    // Pins: a pin per pad of the symbol pins, then extra pads; IDs default to the pad numbers
    // (see `default_pin_ids` for duplicate pins).
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
    pins.extend(options.extra_pads.iter().map(|pad| PinDef {
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
    rename_pins(&mut pins, &common.pins, "pin")?;
    let ids = PinIds::new(&pins, &symbol);
    // The pads of a stacked pin notation, then pins at the same position.
    let mut internal_connections: Vec<Vec<String>> = symbol
        .pins
        .iter()
        .filter(|pin| pin.pads.len() > 1)
        .map(|pin| ids.of(&pin.pads))
        .collect();
    internal_connections.extend(symbol.stacked_pins().iter().map(|group| ids.of_pins(group)));
    // The duplicate pins of each pad.
    let mut pins_of_pad: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for pin in &pins {
        pins_of_pad
            .entry(pin.pad.as_str())
            .or_default()
            .push(pin.id.clone());
    }
    internal_connections.extend(pins_of_pad.into_values().filter(|ids| ids.len() > 1));
    internal_connections.extend(check_internal_connections(
        &common.internal_connections,
        &pins,
    )?);

    let mut fields = options.fields.clone();
    let spice = match (options.spice, options.sim.is_empty()) {
        (SpiceOption::None | SpiceOption::Excluded, false) => {
            return err(
                Spice,
                "`sim` gives a model, so the part cannot be `spice: none` or `spice: excluded`",
            );
        }
        (SpiceOption::None, true) => SpiceDef::Unsupported,
        (SpiceOption::Excluded, true) => SpiceDef::Excluded,
        (SpiceOption::Auto, false) => {
            let (elements, unused_pins) = sim_block(
                &options.sim,
                common.sim_model,
                &pins,
                &internal_connections,
                resolver,
            )?;
            fields.extend(sim_fields(&options.sim, &elements));
            ids.elements(elements, unused_pins)
        }
        (SpiceOption::Auto, true) => {
            let input = SimInput {
                symbol: &symbol,
                ref_prefix: &ref_base,
                value: &value_with(&fields)?,
                builtin_model_name: &format!("__{}", common.type_name),
                model_mode: common.sim_model,
            };
            let def = sim::spice_def(&input, resolver).map_err(|e| PartError {
                key: Spice,
                error: e.context(format!(
                    "cannot simulate {symbol_id} (use `spice: none` or `spice: excluded` to skip)"
                )),
            })?;
            match def {
                SpiceDef::Elements {
                    elements,
                    unused_pins,
                } => {
                    // The model connects symbol pins: the other pads of a stacked pin notation
                    // and extra pads are not part of it.
                    // (Duplicate pins are connected to the first, which the model connects.)
                    let mut numbers = BTreeSet::new();
                    let other_pads = symbol
                        .pins
                        .iter()
                        .filter(|pin| numbers.insert(&pin.number))
                        .filter(|pin| !unused_pins.contains(&pin.number))
                        .flat_map(|pin| pin.pads[1..].iter().cloned());
                    let mut unused_pins: Vec<String> = ids
                        .symbol_pin_pads(&unused_pins)
                        .into_iter()
                        .chain(other_pads)
                        .collect();
                    unused_pins.extend(options.extra_pads.iter().cloned());
                    ids.elements(ids.with_first_pads(elements), unused_pins)
                }
                other => other,
            }
        }
    };

    let value = value_with(&fields)?;
    let non_empty = |name: &str| {
        symbol
            .property(name)
            .filter(|v| !v.is_empty() && *v != "~")
            .map(str::to_string)
    };
    Ok(PartDef {
        kicad: Some(KicadDef {
            symbol: symbol_id,
            footprint,
            fields,
            exclude_from_bom: !symbol.in_bom,
            exclude_from_board: !symbol.on_board,
            description: non_empty("Description"),
            datasheet: non_empty("Datasheet"),
        }),
        ref_base,
        value,
        pins,
        internal_connections,
        spice,
    })
}

/// Expands the text variables of a value (e.g. `${SIM.PARAMS}`) as KiCad does from the fields of
/// the symbol in the schematic: `fields` of the part, the footprint, and the symbol's properties,
/// whose `Sim.*` fields are replaced by the `sim` block. (`Reference` is assigned when the circuit is
/// built, so `${REFERENCE}` is left as it is.)
fn expand_value(
    value: &str,
    fields: &[(String, String)],
    symbol: &Symbol,
    footprint: Option<&LibId>,
    sim: &[ModelBinding],
) -> Result<sim::Value, Error> {
    let footprint = footprint.map(LibId::to_string).unwrap_or_default();
    let properties = symbol.properties.iter().filter(|(name, _)| {
        !(name == "Reference"
            || name == "Value"
            || name == "Footprint"
            || name.starts_with("ki_")
            || (!sim.is_empty() && name.starts_with("Sim.")))
    });
    let all = fields
        .iter()
        .chain(properties)
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .chain([("Footprint", footprint.as_str())]);
    text_var::expand_field(value, all).map(sim::Value::Fixed)
}

/// Pin IDs by pad number (of the first pin of duplicate pins), and pads by symbol pin number.
struct PinIds<'a> {
    ids: BTreeMap<&'a str, &'a str>,
    pads: BTreeMap<&'a str, &'a [String]>,
}

impl<'a> PinIds<'a> {
    fn new(pins: &'a [PinDef], symbol: &'a Symbol) -> Self {
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
    fn of(&self, pads: &[String]) -> Vec<String> {
        pads.iter()
            .map(|n| self.ids[n.as_str()].to_string())
            .collect()
    }

    /// The IDs of symbol pins (of the first pad of a stacked pin notation).
    fn of_pins(&self, numbers: &[String]) -> Vec<String> {
        self.of(&self.first_pads(numbers))
    }

    fn first_pads(&self, numbers: &[String]) -> Vec<String> {
        numbers
            .iter()
            .map(|n| self.pads[n.as_str()][0].clone())
            .collect()
    }

    /// All pads of symbol pins.
    fn symbol_pin_pads(&self, numbers: &[String]) -> Vec<String> {
        numbers
            .iter()
            .flat_map(|n| self.pads[n.as_str()].iter().cloned())
            .collect()
    }

    /// Elements with symbol pin numbers turned into pads (the first of a stacked pin notation,
    /// whose pads are connected).
    fn with_first_pads(&self, mut elements: Vec<sim::ElementDef>) -> Vec<sim::ElementDef> {
        for element in &mut elements {
            element.nodes = self.first_pads(&element.nodes);
        }
        elements
    }

    /// The SPICE description with pin IDs, from one with pad numbers.
    fn elements(&self, mut elements: Vec<sim::ElementDef>, unused_pins: Vec<String>) -> SpiceDef {
        for element in &mut elements {
            element.nodes = self.of(&element.nodes);
        }
        SpiceDef::Elements {
            elements,
            unused_pins: self.of(&unused_pins),
        }
    }
}

/// The footprint of the part, checked against the symbol pins and the extra pads.
fn footprint(
    options: &PartOptions,
    symbol: &Symbol,
    symbol_id: &LibId,
    resolver: &mut Resolver,
) -> Result<Option<LibId>, PartError> {
    use OptionKey::{ExtraPads, Footprint};
    let text = match &options.footprint {
        FootprintOption::Given(text) => text.clone(),
        FootprintOption::FromSymbol => match symbol.property("Footprint").filter(|f| !f.is_empty())
        {
            Some(text) => text.to_string(),
            None => {
                return err(
                    Footprint,
                    format!(
                        "symbol {symbol_id} has no footprint; give one with `footprint: \"Library:Name\"`"
                    ),
                );
            }
        },
        FootprintOption::None => {
            if !options.extra_pads.is_empty() {
                return err(ExtraPads, "extra pads need a footprint");
            }
            return Ok(None);
        }
    };
    let id = text.parse::<LibId>().map_err(|e| at(Footprint)(e.into()))?;
    let pads = resolver
        .footprint(id.library(), id.name())
        .map_err(at(Footprint))?;
    let symbol_pads: BTreeSet<&str> = symbol
        .pins
        .iter()
        .flat_map(|pin| &pin.pads)
        .map(String::as_str)
        .collect();
    let missing: Vec<&str> = symbol
        .pins
        .iter()
        .flat_map(|pin| &pin.pads)
        .map(String::as_str)
        .filter(|pad| !pads.has_pad(pad))
        .collect();
    if !missing.is_empty() {
        return err(
            Footprint,
            format!(
                "footprint {id} has no pads for the symbol pins {}",
                missing.join(", ")
            ),
        );
    }
    for pad in &options.extra_pads {
        if symbol_pads.contains(pad.as_str()) {
            return err(ExtraPads, format!("pad {pad:?} is a pin of the symbol"));
        }
        if !pads.has_pad(pad) {
            return err(ExtraPads, format!("footprint {id} has no pad {pad:?}"));
        }
    }
    let unlisted: Vec<&str> = pads
        .pad_numbers
        .iter()
        .map(String::as_str)
        .filter(|pad| !symbol_pads.contains(pad) && !options.extra_pads.iter().any(|p| p == pad))
        .collect();
    if !unlisted.is_empty() {
        return err(
            Footprint,
            format!(
                "footprint {id} has pads without a symbol pin: {}; list them in `extra_pads` \
                 (and connect them, e.g. with `internal_connections`)",
                unlisted.join(", ")
            ),
        );
    }
    Ok(Some(id))
}

/// Binds the models of the `sim` block, with pad numbers.
fn sim_block(
    bindings: &[ModelBinding],
    mode: ModelMode,
    pins: &[PinDef],
    internal_connections: &[Vec<String>],
    resolver: &mut Resolver,
) -> Result<binding::Bound, PartError> {
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
    let mut pads = BTreeSet::new();
    let part_pins: Vec<PartPin> = pins
        .iter()
        .filter(|pin| pads.insert(pin.pad.as_str()))
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
fn sim_fields(bindings: &[ModelBinding], elements: &[sim::ElementDef]) -> Vec<(String, String)> {
    let ([binding], [element]) = (bindings, elements) else {
        return Vec::new();
    };
    let Some(info) = &element.info else {
        return Vec::new();
    };
    let pins: Vec<String> = element
        .nodes
        .iter()
        .zip(&info.ports)
        .map(|(pin, port)| format!("{pin}={port}"))
        .collect();
    let mut fields = vec![
        ("Sim.Device".to_string(), info.sim_device.to_string()),
        ("Sim.Library".to_string(), library_field(&info.path)),
        ("Sim.Name".to_string(), info.name.clone()),
        ("Sim.Pins".to_string(), pins.join(" ")),
    ];
    if !binding.params.is_empty() {
        let params: Vec<String> = binding
            .params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        fields.push(("Sim.Params".to_string(), params.join(" ")));
    }
    fields
}

/// Makes a part from a SPICE model: its pins are the ports of the `.subckt` (or the terminals of the
/// `.model`). It has no KiCad information.
pub fn load_spice_part(
    options: &SpicePartOptions,
    resolver: &mut Resolver,
) -> Result<PartDef, PartError> {
    let common = &options.common;
    let bind_options = BindOptions {
        pins: PartPins::Ports,
        strict: true,
        internal_connections: &[],
        mode: common.sim_model,
    };
    let model = ModelBinding {
        pins: Vec::new(),
        unused: Vec::new(),
        suffix: String::new(),
        ..options.model.clone()
    };
    let (mut elements, _) =
        binding::bind(&[model], &bind_options, resolver).map_err(model_error)?;
    let info = elements[0].info.clone().expect("a library model");
    let mut pins: Vec<PinDef> = info
        .ports
        .iter()
        .map(|port| PinDef {
            id: port.clone(),
            pad: port.clone(),
            name: None,
            electrical_type: ElectricalType::Passive,
            unit: 0,
        })
        .collect();
    rename_pins(&mut pins, &common.pins, "port")?;
    elements[0].nodes = pins.iter().map(|pin| pin.id.clone()).collect();
    let internal_connections = check_internal_connections(&common.internal_connections, &pins)?;
    let ref_base = match (&common.ref_base, elements[0].device) {
        (Some(ref_base), _) => ref_base.clone(),
        (None, 'X') => "U".to_string(),
        (None, 'D') => "D".to_string(),
        (None, _) => "Q".to_string(),
    };
    check_ref_base(&ref_base)?;
    Ok(PartDef {
        kicad: None,
        ref_base,
        value: common.value.clone().unwrap_or(Value::Fixed(info.name)),
        pins,
        internal_connections,
        spice: SpiceDef::Elements {
            elements,
            unused_pins: Vec::new(),
        },
    })
}

fn check_ref_base(ref_base: &str) -> Result<(), PartError> {
    if !ref_base.starts_with(|c: char| c.is_ascii_alphabetic())
        || !ref_base
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return err(
            OptionKey::RefBase,
            format!("invalid reference prefix {ref_base:?}"),
        );
    }
    Ok(())
}

/// Applies `pins` (pin ID by pad number or port name; ports are matched case-insensitively) and
/// checks that the IDs are unique.
fn rename_pins(
    pins: &mut [PinDef],
    renames: &[(String, String)],
    what: &str,
) -> Result<(), PartError> {
    let default_ids: Vec<String> = pins.iter().map(|pin| pin.id.clone()).collect();
    let mut renamed = BTreeSet::new();
    for (key, id) in renames {
        let normalized = if what == "port" {
            key.to_ascii_lowercase()
        } else {
            key.clone()
        };
        if !renamed.insert(normalized) {
            return err(OptionKey::Pins, format!("{what} {key:?} is renamed twice"));
        }
        if id.is_empty() {
            return err(OptionKey::Pins, format!("empty pin ID for {what} {key:?}"));
        }
        // A pad number stands for its first pin; the other duplicate pins are given by their
        // default IDs (`6#2`).
        let index = if what == "port" {
            pins.iter()
                .position(|pin| pin.pad.eq_ignore_ascii_case(key))
        } else {
            pins.iter()
                .position(|pin| &pin.pad == key)
                .or_else(|| default_ids.iter().position(|id| id == key))
        };
        let pin = index.map(|i| &mut pins[i]);
        match pin {
            Some(pin) => pin.id = id.clone(),
            None => {
                let known: Vec<&str> = pins.iter().map(|pin| pin.pad.as_str()).collect();
                return err(
                    OptionKey::Pins,
                    if what == "port" {
                        format!(
                            "the model has no port {key:?}; its ports are {}",
                            known.join(" ")
                        )
                    } else {
                        format!("the part has no pin {key:?}")
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

/// The default pin IDs: the pad numbers, with `#2`, `#3`, ... appended for the second and later
/// duplicate pins of a pad (the first is the one KiCad writes to the netlist).
fn default_pin_ids(pins: &[PinDef]) -> Vec<String> {
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

fn check_internal_connections(
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

fn check_symbol(symbol: &Symbol) -> Result<(), Error> {
    if symbol.power {
        return Err(Error::new(
            "power symbols are not parts; use a global net label (NetLabel::global) instead",
        ));
    }
    if symbol.pins.is_empty() {
        return Err(Error::new("the symbol has no pins"));
    }
    let hidden_power: Vec<&str> = symbol
        .pins
        .iter()
        .filter(|pin| pin.hidden && pin.electrical_type == ElectricalType::PowerIn)
        .map(|pin| pin.number.as_str())
        .collect();
    if !hidden_power.is_empty() {
        return Err(Error::new(format!(
            "hidden power input pins ({}) are connected to global nets implicitly by KiCad; \
             this is not supported",
            hidden_power.join(", ")
        )));
    }
    Ok(())
}

/// The `Sim.Library` field of a model file: with `/` as the separator on Windows, as KiCad writes
/// paths and as the SPICE netlist does.
fn library_field(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sim::{Args, ElementDef},
        spice_lib::Dialect,
        test_util::{fixture_resolver, strings},
    };

    fn resolver() -> Resolver {
        let mut resolver = fixture_resolver();
        resolver.symbol_dir = Some("symbols".into());
        resolver.footprint_dir = Some("footprints".into());
        resolver
    }

    fn options(symbol: &str, footprint: &str) -> PartOptions {
        PartOptions {
            common: CommonOptions {
                type_name: "Test".into(),
                ..Default::default()
            },
            symbol: symbol.into(),
            footprint: FootprintOption::Given(footprint.into()),
            ..Default::default()
        }
    }

    #[test]
    fn test_resistor() {
        let mut opts = options("test:R", "test:R_0603");
        opts.common.value = Some(Value::Param);
        let mut r = resolver();
        let part = load_part(&opts, &mut r).unwrap();
        assert_eq!(part.ref_base, "R");
        assert_eq!(
            part.kicad.unwrap().footprint.unwrap().to_string(),
            "test:R_0603"
        );
        assert_eq!(
            part.pins.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["1", "2"]
        );
        assert_eq!(
            part.spice,
            SpiceDef::Elements {
                elements: vec![ElementDef {
                    device: 'R',
                    suffix: String::new(),
                    nodes: strings(&["1", "2"]),
                    args: Args::Value { prefix: "".into() },
                    models: vec![],
                    dialect: Dialect::Ngspice,
                    info: None,
                }],
                unused_pins: vec![],
            }
        );
        assert_eq!(r.files.len(), 2);
    }

    #[test]
    fn test_dual_opamp() {
        let mut opts = options("test:DUAL_AMP", "test:SOIC-8-EP");
        opts.extra_pads = strings(&["9"]);
        opts.common.pins = vec![("1".into(), "out_a".into()), ("9".into(), "ep".into())];
        opts.common.internal_connections = vec![strings(&["4", "ep"])];
        let part = load_part(&opts, &mut resolver()).unwrap();
        assert_eq!(part.ref_base, "U");
        assert_eq!(part.value, Value::Fixed("DUAL_AMP".into()));
        let ids: Vec<_> = part.pins.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["out_a", "2", "3", "4", "5", "6", "7", "8", "ep"]);
        assert_eq!(part.internal_connections, vec![strings(&["4", "ep"])]);
        assert_eq!(part.spice, SpiceDef::Unsupported);
    }

    #[test]
    fn test_errors() {
        let error = |opts: PartOptions| {
            let e = load_part(&opts, &mut resolver()).unwrap_err();
            (e.key, e.error.message)
        };
        let (key, message) = error(options("test:DUAL_AMP", "test:SOIC-8-EP"));
        assert_eq!(key, OptionKey::Footprint);
        assert!(
            message.contains("pads without a symbol pin: 9"),
            "{message}"
        );
        let (key, message) = error(options("test:DUAL_AMP", "test:R_0603"));
        assert_eq!(key, OptionKey::Footprint);
        assert!(
            message.contains("no pads for the symbol pins 3, 4"),
            "{message}"
        );
        let (key, message) = error(options("test:GND", "test:R_0603"));
        assert_eq!(key, OptionKey::Symbol);
        assert!(message.contains("power symbols"), "{message}");
        let (key, message) = error(options("test:NOPE", "test:R_0603"));
        assert_eq!(key, OptionKey::Symbol);
        assert!(message.contains("\"NOPE\" not found"), "{message}");
        let (key, _) = error(options("test:R", "test:NOPE"));
        assert_eq!(key, OptionKey::Footprint);
        let mut opts = options("test:R", "test:R_0603");
        opts.common.pins = vec![("1".into(), "a".into()), ("2".into(), "a".into())];
        assert_eq!(error(opts).1, "pin ID \"a\" is used twice");
        let mut opts = options("test:R", "test:R_0603");
        opts.common.value = Some(Value::Fixed("R".into()));
        let (key, message) = error(opts);
        assert_eq!(key, OptionKey::Spice);
        assert!(message.starts_with("cannot simulate test:R"), "{message}");
    }

    #[test]
    fn test_symbol_footprint_property_and_stacked_pins() {
        let mut opts = options("test:REG", "");
        opts.footprint = FootprintOption::FromSymbol;
        let part = load_part(&opts, &mut resolver()).unwrap();
        assert_eq!(
            part.kicad
                .as_ref()
                .unwrap()
                .footprint
                .as_ref()
                .unwrap()
                .to_string(),
            "test:SOT-223"
        );
        assert_eq!(part.internal_connections, vec![strings(&["2", "4"])]);
        assert_eq!(part.spice, SpiceDef::Excluded);
        assert!(part.kicad.unwrap().exclude_from_bom);
    }

    #[test]
    fn test_duplicate_pins() {
        let mut opts = options("test:DUP_AMP", "");
        opts.footprint = FootprintOption::None;
        let part = load_part(&opts, &mut resolver()).unwrap();
        // The first pin of pad 4 is SUB (left of V-), which keeps the pad number as its ID.
        let pins: Vec<_> = part
            .pins
            .iter()
            .map(|p| (p.id.as_str(), p.pad.as_str(), p.name.as_deref()))
            .collect();
        assert_eq!(
            pins,
            [
                ("1", "1", None),
                ("2", "2", Some("-")),
                ("3", "3", Some("+")),
                ("4", "4", Some("SUB")),
                ("4#2", "4", Some("V-")),
                ("5", "5", Some("V+")),
            ]
        );
        assert_eq!(part.internal_connections, vec![strings(&["4", "4#2"])]);
        // The model connects the first pin; the other is connected to it inside the part.
        let SpiceDef::Elements {
            elements,
            unused_pins,
        } = &part.spice
        else {
            panic!("{:?}", part.spice)
        };
        assert_eq!(elements[0].nodes, strings(&["3", "2", "5", "4", "1"]));
        assert!(unused_pins.is_empty());

        // The pad number renames the first pin, and `4#2` the other.
        opts.common.pins = vec![("4#2".into(), "vee".into()), ("4".into(), "sub".into())];
        let part = load_part(&opts, &mut resolver()).unwrap();
        let ids: Vec<_> = part.pins.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["1", "2", "3", "sub", "vee", "5"]);
        assert_eq!(part.internal_connections, vec![strings(&["sub", "vee"])]);
        let SpiceDef::Elements { elements, .. } = &part.spice else {
            panic!("{:?}", part.spice)
        };
        assert_eq!(elements[0].nodes, strings(&["3", "2", "5", "sub", "1"]));
    }

    #[test]
    fn test_stacked_pin_notation() {
        let mut opts = options("test:STACKED_AMP", "");
        opts.footprint = FootprintOption::FromSymbol;
        let part = load_part(&opts, &mut resolver()).unwrap();
        let ids: Vec<_> = part.pins.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["1", "2", "3", "5", "4", "6"]);
        assert_eq!(part.pins[5].name.as_deref(), Some("V-"));
        assert_eq!(part.internal_connections, vec![strings(&["4", "6"])]);
        // The model sees the stacked pin as one pin (Sim.Pins `[4,6]`), connected to pad 4.
        let SpiceDef::Elements {
            elements,
            unused_pins,
        } = &part.spice
        else {
            panic!("{:?}", part.spice)
        };
        assert_eq!(elements[0].nodes, strings(&["3", "2", "5", "4", "1"]));
        assert_eq!(unused_pins, &strings(&["6"]));

        // Pads of the stacked pin are checked against the footprint.
        let error = load_part(
            &options("test:STACKED_AMP", "test:SOT-223"),
            &mut resolver(),
        )
        .unwrap_err();
        assert!(
            error
                .error
                .message
                .contains("no pads for the symbol pins 5, 6"),
            "{}",
            error.error
        );
    }

    fn vsup_binding() -> ModelBinding {
        let pins = [
            ("1", "CTR"),
            ("2", "CTS"),
            ("3", "GND"),
            ("4", "VDD"),
            ("5", "SENSE"),
            ("6", "RESET_B"),
        ];
        ModelBinding {
            library: "models/vsup01.lib".into(),
            name: "VSUP01".into(),
            pins: pins
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn test_value_text_variables() {
        let value = |opts: &PartOptions| load_part(opts, &mut resolver()).unwrap().value;
        let mut opts = options("test:SIM_AMP", "test:SOT-23-6");
        opts.fields = vec![("MPN".into(), "X1".into())];
        opts.common.value = Some(Value::Fixed(
            "${SIM.PARAMS} ${footprint} ${MPN} ${SIM.TYPE}${REFERENCE}".into(),
        ));
        assert_eq!(
            value(&opts),
            Value::Fixed("GAIN=100 test:SOT-23-6 X1 ${REFERENCE}".into())
        );
        // The `sim` block replaces the symbol's `Sim.*` fields.
        let mut opts = options("test:VSUP", "test:SOT-23-6");
        opts.sim = vec![vsup_binding()];
        opts.common.value = Some(Value::Fixed("${SIM.NAME}${SIM.PARAMS}".into()));
        assert_eq!(value(&opts), Value::Fixed("VSUP01".into()));
        let mut opts = options("test:SIM_AMP", "test:SOT-23-6");
        opts.common.value = Some(Value::Fixed("${SIM.PARAMS".into()));
        let error = load_part(&opts, &mut resolver()).unwrap_err();
        assert_eq!(error.key, OptionKey::Value);
    }

    #[test]
    fn test_sim_block() {
        let mut opts = options("test:VSUP", "test:SOT-23-6");
        opts.common.pins = vec![("6".into(), "reset_n".into())];
        opts.sim = vec![vsup_binding()];
        let mut r = resolver();
        let part = load_part(&opts, &mut r).unwrap();
        let SpiceDef::Elements {
            elements,
            unused_pins,
        } = &part.spice
        else {
            panic!()
        };
        // In the order of the .subckt ports, with pin IDs.
        assert_eq!(
            elements[0].nodes,
            strings(&["1", "2", "3", "reset_n", "5", "4"])
        );
        assert_eq!(elements[0].dialect, crate::spice_lib::Dialect::Pspice);
        assert!(unused_pins.is_empty());
        let fields = &part.kicad.unwrap().fields;
        let lib = r.manifest_dir.join("models/vsup01.lib");
        assert_eq!(
            fields,
            &vec![
                ("Sim.Device".to_string(), "SUBCKT".to_string()),
                ("Sim.Library".to_string(), library_field(&lib)),
                ("Sim.Name".to_string(), "VSUP01".to_string()),
                (
                    "Sim.Pins".to_string(),
                    "1=CTR 2=CTS 3=GND 6=RESET_B 5=SENSE 4=VDD".to_string()
                ),
            ]
        );

        // Errors point at the model's option; a pin left out is an error.
        let mut opts = options("test:VSUP", "test:SOT-23-6");
        let mut binding = vsup_binding();
        binding.pins.pop();
        binding.unused = strings(&["6"]);
        opts.sim = vec![binding];
        let e = load_part(&opts, &mut resolver()).unwrap_err();
        assert_eq!(
            e.key,
            OptionKey::Model {
                element: 0,
                field: BindingField::Pins
            }
        );
        assert!(
            e.error
                .message
                .starts_with("no pin is mapped to the ports RESET_B"),
            "{}",
            e.error
        );
        let mut opts = options("test:VSUP", "test:SOT-23-6");
        opts.sim = vec![vsup_binding()];
        opts.spice = SpiceOption::None;
        assert_eq!(
            load_part(&opts, &mut resolver()).unwrap_err().key,
            OptionKey::Spice
        );
    }

    #[test]
    fn test_spice_part() {
        let opts = SpicePartOptions {
            common: CommonOptions {
                pins: vec![("reset_b".into(), "reset_n".into())],
                ..Default::default()
            },
            model: ModelBinding {
                library: "models/vsup01.lib".into(),
                name: "vsup01".into(),
                params: vec![("VTH".into(), "0.6".into())],
                ..Default::default()
            },
        };
        let part = load_spice_part(&opts, &mut resolver()).unwrap();
        assert!(part.kicad.is_none());
        assert_eq!(part.ref_base, "U");
        assert_eq!(part.value, Value::Fixed("VSUP01".into()));
        let ids: Vec<_> = part.pins.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["CTR", "CTS", "GND", "reset_n", "SENSE", "VDD"]);
        let SpiceDef::Elements { elements, .. } = &part.spice else {
            panic!()
        };
        assert_eq!(elements[0].nodes, ids);
        assert_eq!(elements[0].args, Args::Fixed("VSUP01 VTH=0.6".into()));

        let mut opts = opts;
        opts.common.pins = vec![("RESET".into(), "r".into())];
        let e = load_spice_part(&opts, &mut resolver()).unwrap_err();
        assert_eq!(
            (e.key, e.error.message.as_str()),
            (
                OptionKey::Pins,
                "the model has no port \"RESET\"; its ports are CTR CTS GND RESET_B SENSE VDD"
            )
        );
        opts.common.pins.clear();
        opts.model.name = "clamp".into();
        opts.model.library = "models/clamp.lib".into();
        opts.model.params.clear();
        let part = load_spice_part(&opts, &mut resolver()).unwrap();
        assert_eq!((part.ref_base.as_str(), part.pins.len()), ("D", 2));
    }
}
