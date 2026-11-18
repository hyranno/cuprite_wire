//! Turning a symbol (and a footprint) into the description of a part, as the macro generates it.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Error,
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
            Some((library, name)) if !library.is_empty() && !name.is_empty() && !name.contains(':') => Ok(LibId {
                library: library.to_string(),
                name: name.to_string(),
            }),
            _ => Err(Error::new(format!("expected 'Library:Name', found {text:?}"))),
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
    /// From the `Sim.*` fields; an error if they cannot be converted.
    #[default]
    Auto,
    /// Not simulated: `spice()` returns `None`.
    None,
    /// Excluded from the simulation.
    Excluded,
}

/// The options of the macro.
#[derive(Debug, Clone, Default)]
pub struct PartOptions {
    /// `Library:Name`.
    pub symbol: String,
    pub footprint: FootprintOption,
    /// `None`: the symbol's `Value`.
    pub value: Option<Value>,
    pub ref_base: Option<String>,
    /// Pin ID by pin (or extra pad) number.
    pub pins: Vec<(String, String)>,
    /// Groups of pin IDs connected inside the part, in addition to stacked pins.
    pub internal_connections: Vec<Vec<String>>,
    /// Footprint pads without a symbol pin that become pins of the part.
    pub extra_pads: Vec<String>,
    pub spice: SpiceOption,
    pub sim_model: ModelMode,
    pub fields: Vec<(String, String)>,
    /// Name of the generated type, used to name built-in SPICE models.
    pub type_name: String,
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

/// A pin of the generated part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinDef {
    pub id: String,
    /// Footprint pad number.
    pub pad: String,
    pub name: Option<String>,
    pub electrical_type: ElectricalType,
    /// The symbol unit (0: shared, or an extra pad).
    pub unit: u32,
}

/// Everything the macro needs to generate a part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartDef {
    pub symbol: LibId,
    pub footprint: Option<LibId>,
    pub ref_base: String,
    pub value: Value,
    pub pins: Vec<PinDef>,
    /// Groups of pin IDs.
    pub internal_connections: Vec<Vec<String>>,
    pub fields: Vec<(String, String)>,
    pub exclude_from_bom: bool,
    pub exclude_from_board: bool,
    pub description: Option<String>,
    pub datasheet: Option<String>,
    /// The SPICE description, with pin IDs.
    pub spice: SpiceDef,
}

pub fn load_part(options: &PartOptions, resolver: &mut Resolver) -> Result<PartDef, PartError> {
    use OptionKey::*;
    let symbol_id = LibId::parse(&options.symbol).map_err(at(Symbol))?;
    let lib = resolver.symbol_lib(&symbol_id.library).map_err(at(Symbol))?;
    let symbol = lib.symbol(&symbol_id.name).map_err(at(Symbol))?;
    check_symbol(&symbol).map_err(at(Symbol))?;

    let ref_base = match &options.ref_base {
        Some(ref_base) => ref_base.clone(),
        None => symbol
            .property("Reference")
            .unwrap_or_default()
            .trim_end_matches(|c: char| c == '?' || c.is_ascii_digit())
            .to_string(),
    };
    if !ref_base.starts_with(|c: char| c.is_ascii_alphabetic())
        || !ref_base.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return err(RefBase, format!("invalid reference prefix {ref_base:?}"));
    }
    let value = match &options.value {
        Some(value) => value.clone(),
        None => sim::Value::Fixed(symbol.property("Value").unwrap_or_default().to_string()),
    };

    // Footprint and pads.
    let footprint_text = match &options.footprint {
        FootprintOption::Given(text) => Some(text.clone()),
        FootprintOption::FromSymbol => match symbol.property("Footprint").filter(|f| !f.is_empty()) {
            Some(text) => Some(text.to_string()),
            None => {
                return err(
                    Footprint,
                    format!("symbol {symbol_id} has no footprint; give one with `footprint: \"Library:Name\"`"),
                );
            }
        },
        FootprintOption::None => None,
    };
    let footprint = match footprint_text {
        None => {
            if !options.extra_pads.is_empty() {
                return err(ExtraPads, "extra pads need a footprint");
            }
            None
        }
        Some(text) => {
            let id = LibId::parse(&text).map_err(at(Footprint))?;
            let pads = resolver.footprint(&id.library, &id.name).map_err(at(Footprint))?;
            let missing: Vec<&str> = symbol
                .pins
                .iter()
                .map(|pin| pin.number.as_str())
                .filter(|number| !pads.has_pad(number))
                .collect();
            if !missing.is_empty() {
                return err(
                    Footprint,
                    format!("footprint {id} has no pads for the symbol pins {}", missing.join(", ")),
                );
            }
            let symbol_numbers: BTreeSet<&str> = symbol.pins.iter().map(|pin| pin.number.as_str()).collect();
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
            Some(id)
        }
    };

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
    let mut renamed = BTreeSet::new();
    for (number, id) in &options.pins {
        if !renamed.insert(number) {
            return err(Pins, format!("pin {number:?} is renamed twice"));
        }
        if id.is_empty() {
            return err(Pins, format!("empty pin ID for pin {number:?}"));
        }
        match pins.iter_mut().find(|pin| &pin.pad == number) {
            Some(pin) => pin.id = id.clone(),
            None => return err(Pins, format!("the part has no pin {number:?}")),
        }
    }
    let mut ids = BTreeSet::new();
    for pin in &pins {
        if !ids.insert(pin.id.as_str()) {
            return err(Pins, format!("pin ID {:?} is used twice", pin.id));
        }
    }
    let id_of: BTreeMap<&str, &str> = pins.iter().map(|pin| (pin.pad.as_str(), pin.id.as_str())).collect();
    let to_ids = |numbers: &[String]| -> Vec<String> { numbers.iter().map(|n| id_of[n.as_str()].to_string()).collect() };

    let mut internal_connections: Vec<Vec<String>> = symbol.stacked_pins().iter().map(|group| to_ids(group)).collect();
    for group in &options.internal_connections {
        if group.len() < 2 {
            return err(InternalConnections, "a group needs at least two pins");
        }
        if let Some(unknown) = group.iter().find(|id| !ids.contains(id.as_str())) {
            return err(InternalConnections, format!("the part has no pin ID {unknown:?}"));
        }
        internal_connections.push(group.clone());
    }

    let spice = match options.spice {
        SpiceOption::None => SpiceDef::Unsupported,
        SpiceOption::Excluded => SpiceDef::Excluded,
        SpiceOption::Auto => {
            let input = SimInput {
                symbol: &symbol,
                ref_prefix: &ref_base,
                value: &value,
                builtin_model_name: &format!("__{}", options.type_name),
                model_mode: options.sim_model,
            };
            let def = sim::spice_def(&input, resolver).map_err(|e| PartError {
                key: Spice,
                error: e.context(format!(
                    "cannot simulate {symbol_id} (use `spice: none` or `spice: excluded` to skip)"
                )),
            })?;
            match def {
                SpiceDef::Element(mut element) => {
                    element.nodes = to_ids(&element.nodes);
                    element.unused_pins = to_ids(&element.unused_pins);
                    // Extra pads are not part of the model.
                    element
                        .unused_pins
                        .extend(options.extra_pads.iter().map(|pad| id_of[pad.as_str()].to_string()));
                    SpiceDef::Element(element)
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
        symbol: symbol_id,
        footprint,
        ref_base,
        value,
        pins,
        internal_connections,
        fields: options.fields.clone(),
        exclude_from_bom: !symbol.in_bom,
        exclude_from_board: !symbol.on_board,
        description: non_empty("Description"),
        datasheet: non_empty("Datasheet"),
        spice,
    })
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
    use crate::sim::{Args, ElementDef};

    fn resolver() -> Resolver {
        let mut resolver = Resolver::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"), |_| None);
        resolver.symbol_dir = Some("symbols".into());
        resolver.footprint_dir = Some("footprints".into());
        resolver
    }

    fn options(symbol: &str, footprint: &str) -> PartOptions {
        PartOptions {
            symbol: symbol.into(),
            footprint: FootprintOption::Given(footprint.into()),
            type_name: "Test".into(),
            ..Default::default()
        }
    }

    fn strings(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_resistor() {
        let mut opts = options("test:R", "test:R_0603");
        opts.value = Some(Value::Param);
        let mut r = resolver();
        let part = load_part(&opts, &mut r).unwrap();
        assert_eq!(part.ref_base, "R");
        assert_eq!(part.footprint.unwrap().to_string(), "test:R_0603");
        assert_eq!(part.pins.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["1", "2"]);
        assert_eq!(
            part.spice,
            SpiceDef::Element(ElementDef {
                device: 'R',
                nodes: strings(&["1", "2"]),
                args: Args::Value { prefix: "".into() },
                models: vec![],
                unused_pins: vec![],
            })
        );
        assert_eq!(r.files.len(), 2);
    }

    #[test]
    fn test_dual_opamp() {
        let mut opts = options("test:DUAL_AMP", "test:SOIC-8-EP");
        opts.extra_pads = strings(&["9"]);
        opts.pins = vec![("1".into(), "out_a".into()), ("9".into(), "ep".into())];
        opts.internal_connections = vec![strings(&["4", "ep"])];
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
        assert!(message.contains("pads without a symbol pin: 9"), "{message}");
        let (key, message) = error(options("test:DUAL_AMP", "test:R_0603"));
        assert_eq!(key, OptionKey::Footprint);
        assert!(message.contains("no pads for the symbol pins 3, 4"), "{message}");
        let (key, message) = error(options("test:GND", "test:R_0603"));
        assert_eq!(key, OptionKey::Symbol);
        assert!(message.contains("power symbols"), "{message}");
        let (key, message) = error(options("test:NOPE", "test:R_0603"));
        assert_eq!(key, OptionKey::Symbol);
        assert!(message.contains("\"NOPE\" not found"), "{message}");
        let (key, _) = error(options("test:R", "test:NOPE"));
        assert_eq!(key, OptionKey::Footprint);
        let mut opts = options("test:R", "test:R_0603");
        opts.pins = vec![("1".into(), "a".into()), ("2".into(), "a".into())];
        assert_eq!(error(opts).1, "pin ID \"a\" is used twice");
        let mut opts = options("test:R", "test:R_0603");
        opts.value = Some(Value::Fixed("R".into()));
        let (key, message) = error(opts);
        assert_eq!(key, OptionKey::Spice);
        assert!(message.starts_with("cannot simulate test:R"), "{message}");
    }

    #[test]
    fn test_symbol_footprint_property_and_stacked_pins() {
        let mut opts = options("test:REG", "");
        opts.footprint = FootprintOption::FromSymbol;
        let part = load_part(&opts, &mut resolver()).unwrap();
        assert_eq!(part.footprint.unwrap().to_string(), "test:SOT-223");
        assert_eq!(part.internal_connections, vec![strings(&["2", "4"])]);
        assert_eq!(part.spice, SpiceDef::Excluded);
        assert!(part.exclude_from_bom);
    }
}
