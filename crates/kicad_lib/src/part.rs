//! Turning a symbol (and a footprint), or a SPICE model, into the description of a part, as the
//! macros generate it.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Error,
    binding::{self, BindOptions, BindingField, ModelBinding, PartPin, PartPins},
    resolve::Resolver,
    sim::{self, ModelMode, SimInput, SpiceDef, Value},
    symbol::{ElectricalType, Symbol},
};

/// A `Library:Name` identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibId {
    pub library: String,
    pub name: String,
}

impl LibId {
    pub fn parse(text: &str) -> Result<Self, Error> {
        match text.split_once(':') {
            Some((library, name))
                if !library.is_empty() && !name.is_empty() && !name.contains(':') =>
            {
                Ok(LibId {
                    library: library.to_string(),
                    name: name.to_string(),
                })
            }
            _ => Err(Error::new(format!(
                "expected 'Library:Name', found {text:?}"
            ))),
        }
    }
}

impl std::fmt::Display for LibId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.library, self.name)
    }
}

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
    let symbol_id = LibId::parse(&options.symbol).map_err(at(Symbol))?;
    let lib = resolver
        .symbol_lib(&symbol_id.library)
        .map_err(at(Symbol))?;
    let symbol = lib.symbol(&symbol_id.name).map_err(at(Symbol))?;
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
    let value = match &common.value {
        Some(value) => value.clone(),
        None => sim::Value::Fixed(symbol.property("Value").unwrap_or_default().to_string()),
    };
    let footprint = footprint(options, &symbol, &symbol_id, resolver)?;

    // Pins: symbol pins, then extra pads; IDs default to the numbers.
    let mut pins: Vec<PinDef> = symbol
        .pins
        .iter()
        .map(|pin| PinDef {
            id: pin.number.clone(),
            pad: pin.number.clone(),
            name: pin.name.clone(),
            electrical_type: pin.electrical_type,
            unit: pin.unit,
        })
        .collect();
    pins.extend(options.extra_pads.iter().map(|pad| PinDef {
        id: pad.clone(),
        pad: pad.clone(),
        name: None,
        electrical_type: ElectricalType::Passive,
        unit: 0,
    }));
    rename_pins(&mut pins, &common.pins, "pin")?;
    let ids = PinIds::new(&pins);
    let mut internal_connections: Vec<Vec<String>> = symbol
        .stacked_pins()
        .iter()
        .map(|group| ids.of(group))
        .collect();
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
                value: &value,
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
                    mut unused_pins,
                } => {
                    // Extra pads are not part of the model.
                    unused_pins.extend(options.extra_pads.iter().cloned());
                    ids.elements(elements, unused_pins)
                }
                other => other,
            }
        }
    };

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

/// Pin IDs by pad number.
struct PinIds<'a>(BTreeMap<&'a str, &'a str>);

impl<'a> PinIds<'a> {
    fn new(pins: &'a [PinDef]) -> Self {
        PinIds(
            pins.iter()
                .map(|pin| (pin.pad.as_str(), pin.id.as_str()))
                .collect(),
        )
    }

    fn of(&self, numbers: &[String]) -> Vec<String> {
        numbers
            .iter()
            .map(|n| self.0[n.as_str()].to_string())
            .collect()
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
    let id = LibId::parse(&text).map_err(at(Footprint))?;
    let pads = resolver
        .footprint(&id.library, &id.name)
        .map_err(at(Footprint))?;
    let missing: Vec<&str> = symbol
        .pins
        .iter()
        .map(|pin| pin.number.as_str())
        .filter(|number| !pads.has_pad(number))
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
    let symbol_numbers: BTreeSet<&str> =
        symbol.pins.iter().map(|pin| pin.number.as_str()).collect();
    for pad in &options.extra_pads {
        if symbol_numbers.contains(pad.as_str()) {
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
        .filter(|pad| !symbol_numbers.contains(pad) && !options.extra_pads.iter().any(|p| p == pad))
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
    let part_pins: Vec<PartPin> = pins
        .iter()
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
        (
            "Sim.Library".to_string(),
            info.path.to_string_lossy().into_owned(),
        ),
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
        let pin = if what == "port" {
            pins.iter_mut()
                .find(|pin| pin.pad.eq_ignore_ascii_case(key))
        } else {
            pins.iter_mut().find(|pin| &pin.pad == key)
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sim::{Args, ElementDef},
        spice_lib::Dialect,
    };

    fn resolver() -> Resolver {
        let mut resolver = Resolver::new(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"),
            |_| None,
        );
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

    fn strings(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
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
                (
                    "Sim.Library".to_string(),
                    lib.to_string_lossy().into_owned()
                ),
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
