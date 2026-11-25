//! Turning the `Sim.*` fields of a symbol into the SPICE description of a part.
//!
//! The supported subset of KiCad 9's simulation models:
//! - no `Sim.*` fields: R, C and L inferred from the reference prefix, with the value as is;
//!   other symbols are not simulated (unsupported, not an error);
//! - `exclude_from_sim` / `Sim.Enable 0`: excluded from the simulation;
//! - `Sim.Device` `R` `C` `L`, `V` `I` (`Sim.Type` `DC` `SIN` `PULSE` `EXP` `AM` `SFFM` `PWL`),
//!   `D`, `NPN` `PNP` (Gummel-Poon),
//!   `NMOS` `PMOS` (`VDMOS`, `MOS1`), `NJFET` `PJFET` (Shichman-Hodges), with built-in model
//!   parameters (`Sim.Params`) or a library model (`Sim.Library` + `Sim.Name`);
//! - `Sim.Device SUBCKT` with `Sim.Library` + `Sim.Name` (+ `Sim.Params`);
//! - `Sim.Device IBIS` (KiCad 10, [`ibis`]).
//!
//! Anything else is an error: the macro user decides with `spice: none` / `spice: excluded`.

pub mod ibis;

use std::path::PathBuf;

use crate::{
    Error,
    binding::{
        BindOptions, ModelBinding, ModelInfo, PartPin, PartPins, bind, check_param_value,
        map_ports, model_device,
    },
    resolve::Resolver,
    spice_lib::Dialect,
    symbol::Symbol,
    value,
};

/// The value of a part: fixed at compile time or given when the part is created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Fixed(String),
    Param,
}

/// The SPICE description of a part, with pins identified by their numbers.
#[derive(Debug, Clone, PartialEq)]
pub enum SpiceDef {
    /// The symbol has no simulation model.
    Unsupported,
    Excluded,
    Elements(Elements),
    /// A buffer of an IBIS file, simulated by the part the macro generates.
    Ibis(Box<ibis::IbisDef>),
}

/// The elements of a part, with pins in node order, and the pins not connected to any element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elements {
    pub elements: Vec<ElementDef>,
    /// Pins not connected to the model.
    pub unused_pins: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementDef {
    pub device: char,
    /// Appended to the element name (`XU1A`); empty for a single element.
    pub suffix: String,
    /// Pin numbers in the order of the model's nodes.
    pub nodes: Vec<String>,
    pub args: Args,
    pub models: Vec<ModelDef>,
    /// The dialect of the models.
    pub dialect: Dialect,
    /// The library model, if the element uses one.
    pub info: Option<ModelInfo>,
}

impl ElementDef {
    /// The port of the library model the pin `id` is connected to, written by `show`, with the
    /// suffix of the element if the part has several (`PORT (A)`).
    pub fn port_of(&self, id: &str, show: impl Fn(&str) -> String) -> Option<String> {
        let info = self.info.as_ref()?;
        let index = self.nodes.iter().position(|node| node == id)?;
        let port = show(&info.ports[index]);
        Some(if self.suffix.is_empty() {
            port
        } else {
            format!("{port} ({})", self.suffix)
        })
    }
}

/// The arguments after the nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Args {
    Fixed(String),
    /// The value given when the part is created, between `prefix` and `suffix`.
    Value {
        prefix: String,
        suffix: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelDef {
    /// An absolute path to include.
    Include(PathBuf),
    Inline {
        name: String,
        text: String,
    },
}

/// How library models are referred to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelMode {
    /// `.include` the library file (default).
    #[default]
    Include,
    /// Copy the definitions into the deck.
    Inline,
}

pub struct SimInput<'a> {
    pub symbol: &'a Symbol,
    /// The reference prefix of the part (`R`, `U`, ...).
    pub ref_prefix: &'a str,
    pub value: &'a Value,
    /// Prefix of the name of a built-in model (`.model <prefix>_<hash> D ...`), to which the hash
    /// of the model is appended so that different models never share a name.
    pub builtin_model_name: &'a str,
    pub model_mode: ModelMode,
}

pub fn spice_def(input: &SimInput, resolver: &mut Resolver) -> Result<SpiceDef, Error> {
    let symbol = input.symbol;
    if symbol.exclude_from_sim || symbol.property("Sim.Enable") == Some("0") {
        return Ok(SpiceDef::Excluded);
    }
    let Some(device) = symbol.property("Sim.Device").filter(|d| !d.is_empty()) else {
        return inferred(input);
    };
    let fields = SimFields {
        device: device.to_ascii_uppercase(),
        sim_type: symbol
            .property("Sim.Type")
            .filter(|t| !t.is_empty())
            .map(str::to_ascii_uppercase),
        params: parse_params(symbol.property("Sim.Params").unwrap_or_default())?,
        library: symbol.property("Sim.Library").filter(|l| !l.is_empty()),
        model_name: symbol.property("Sim.Name").filter(|n| !n.is_empty()),
    };
    for (name, value) in &fields.params {
        // `pwl` of the sources is a list of numbers, each checked by `pwl_points`.
        if !(matches!(fields.device.as_str(), "V" | "I") && name.eq_ignore_ascii_case("pwl")) {
            check_param_value(name, value)?;
        }
    }
    if fields.device == "IBIS" {
        return Ok(SpiceDef::Ibis(Box::new(ibis::from_fields(
            symbol, &fields, resolver,
        )?)));
    }
    let elements = match fields.device.as_str() {
        "R" | "C" | "L" => passive(input, &fields)?,
        "V" | "I" => source(input, &fields)?,
        "D" | "NPN" | "PNP" | "NMOS" | "PMOS" | "NJFET" | "PJFET" => {
            semiconductor(input, &fields, resolver)?
        }
        "SUBCKT" => subcircuit(input, &fields, resolver)?,
        device => return Err(Error::new(format!("Sim.Device {device} is not supported"))),
    };
    Ok(SpiceDef::Elements(elements))
}

/// The `Sim.*` fields of a symbol with a `Sim.Device`.
struct SimFields<'a> {
    /// Uppercase.
    device: String,
    /// Uppercase.
    sim_type: Option<String>,
    params: Vec<(String, String)>,
    library: Option<&'a str>,
    model_name: Option<&'a str>,
}

impl SimFields<'_> {
    fn unsupported_type(&self) -> Error {
        Error::new(format!(
            "Sim.Type {:?} of Sim.Device {} is not supported",
            self.sim_type.as_deref().unwrap_or_default(),
            self.device
        ))
    }
}

/// `Sim.Device` `R` `C` `L`: the value, or the one parameter.
fn passive(input: &SimInput, fields: &SimFields) -> Result<Elements, Error> {
    let device = &fields.device;
    let letter = device.chars().next().unwrap();
    if fields.library.is_some() {
        return Err(Error::new(format!(
            "Sim.Library for Sim.Device {device} is not supported"
        )));
    }
    if fields.sim_type.is_some() {
        return Err(fields.unsupported_type());
    }
    let key = device.to_ascii_lowercase();
    let args = match fields.params.as_slice() {
        [] => value_args(input.value, letter, "", "")?,
        [(name, value)] if name.eq_ignore_ascii_case(&key) => {
            Args::Fixed(param_value(name, value, letter)?)
        }
        _ => {
            return Err(Error::new(format!(
                "Sim.Params {:?} of Sim.Device {device} is not supported (only {key}=...)",
                input.symbol.property("Sim.Params").unwrap_or_default()
            )));
        }
    };
    let (nodes, unused) = map_pins(input.symbol, &["+", "-"])?;
    Ok(element(letter, nodes, args, Vec::new(), unused))
}

/// The `Sim.Type`s of the sources `V` and `I`: the SPICE function and its parameters in order.
/// All of them are followed by `ac` and `ph` (the AC magnitude and phase).
const SOURCE_TYPES: &[(&str, &str, &[&str])] = &[
    ("DC", "", &["dc"]),
    ("SIN", "SIN", &["dc", "ampl", "f", "td", "theta", "phase"]),
    (
        "PULSE",
        "PULSE",
        &["y1", "y2", "td", "tr", "tf", "tw", "per", "np"],
    ),
    ("EXP", "EXP", &["y1", "y2", "td1", "tau1", "td2", "tau2"]),
    (
        "AM",
        "AM",
        &["vo", "vmo", "vma", "fm", "fc", "td", "phasem", "phasec"],
    ),
    (
        "SFFM",
        "SFFM",
        &["vo", "va", "fm", "mdi", "fc", "phasem", "phasec"],
    ),
    ("PWL", "PWL", &["pwl"]),
];

/// The parameters in volts or amperes, which may have the unit `V` or `A`.
const SOURCE_LEVELS: &[&str] = &["dc", "ampl", "y1", "y2", "vo", "vmo", "vma", "va", "ac"];

/// `Sim.Device` `V` `I`: `DC <dc>` or a waveform function of `Sim.Type`, then `AC <ac> <ph>`.
///
/// Written as KiCad 10 writes them (`doc/dev/spice_netlist_format.md` section 3.3): the parameters
/// before the last one given default to `0`, and the ones after it are left out.
fn source(input: &SimInput, fields: &SimFields) -> Result<Elements, Error> {
    let device = &fields.device;
    let letter = device.chars().next().unwrap();
    if fields.library.is_some() {
        return Err(Error::new(format!(
            "Sim.Library for Sim.Device {device} is not supported"
        )));
    }
    let sim_type = fields.sim_type.as_deref().unwrap_or("DC");
    let &(_, function, wave) = SOURCE_TYPES
        .iter()
        .find(|(name, _, _)| *name == sim_type)
        .ok_or_else(|| fields.unsupported_type())?;
    let mut values = source_values(fields, sim_type, wave, letter)?;
    if sim_type == "PULSE" && values[6].is_none() && values[7].is_none() {
        // A single pulse, as KiCad: ngspice 46 repeats a pulse without `per`.
        values[6] = Some("0".into());
        values[7] = Some("1".into());
    }
    // `dc` of a DC source defaults to the value instead.
    let dc_from_value = function.is_empty() && values[0].is_none();
    let given = values
        .iter()
        .rposition(Option::is_some)
        .map_or(0, |i| i + 1);
    let values: Vec<String> = values[..given]
        .iter()
        .map(|value| value.clone().unwrap_or_else(|| "0".into()))
        .collect();
    let ac = match values.get(wave.len()..).unwrap_or_default() {
        [] => String::new(),
        [ac] => format!(" AC {ac}"),
        [ac, ph] => format!(" AC {ac} {ph}"),
        _ => unreachable!(),
    };
    let wave_values = &values[..given.min(wave.len())];
    let args = if function.is_empty() {
        if dc_from_value {
            value_args(input.value, letter, "DC ", &ac)?
        } else {
            Args::Fixed(format!("DC {}{ac}", wave_values[0]))
        }
    } else {
        if fields.params.is_empty() {
            return Err(Error::new(format!(
                "Sim.Type {sim_type} of Sim.Device {device} needs Sim.Params"
            )));
        }
        // SIN's offset `dc` is also the DC value.
        let dc = match (sim_type, wave_values.first()) {
            ("SIN", Some(dc)) => format!("DC {dc} "),
            _ => String::new(),
        };
        Args::Fixed(format!("{dc}{function}( {} ){ac}", wave_values.join(" ")))
    };
    let (nodes, unused) = map_pins(input.symbol, &["+", "-"])?;
    Ok(element(letter, nodes, args, Vec::new(), unused))
}

/// The `Sim.Params` of a source of `sim_type` in SPICE's notation, in the order of the parameters
/// of its waveform `wave`, then `ac` and `ph`.
fn source_values(
    fields: &SimFields,
    sim_type: &str,
    wave: &[&str],
    letter: char,
) -> Result<Vec<Option<String>>, Error> {
    let names: Vec<&str> = wave.iter().copied().chain(["ac", "ph"]).collect();
    let mut values: Vec<Option<String>> = vec![None; names.len()];
    for (name, value) in &fields.params {
        let index = names
            .iter()
            .position(|known| known.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                Error::new(format!(
                    "Sim.Params {name} of Sim.Type {sim_type} is not supported (its parameters are {})",
                    names.join(" ")
                ))
            })?;
        if values[index].is_some() {
            return Err(Error::new(format!("Sim.Params {name} is given twice")));
        }
        values[index] = Some(if names[index] == "pwl" {
            pwl_points(value, letter)?
        } else {
            let unit = if SOURCE_LEVELS.contains(&names[index]) {
                letter
            } else {
                ' '
            };
            param_value(name, value, unit)?
        });
    }
    Ok(values)
}

/// A number of `Sim.Params` in KiCad's notation, converted to SPICE's (with the unit of `device`).
fn param_value(name: &str, value: &str, device: char) -> Result<String, Error> {
    value::to_spice(value, device)
        .ok_or_else(|| Error::new(format!("Sim.Params {name}={value} is not a number")))
}

/// The `pwl` parameter: pairs of time and value, numbers in KiCad's notation.
fn pwl_points(text: &str, device: char) -> Result<String, Error> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() || !tokens.len().is_multiple_of(2) {
        return Err(Error::new(format!(
            "Sim.Params pwl={text:?} is not pairs of time and value"
        )));
    }
    let points = tokens
        .iter()
        .enumerate()
        .map(|(i, token)| param_value("pwl", token, if i.is_multiple_of(2) { ' ' } else { device }))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(points.join(" "))
}

/// Diodes and transistors: a built-in model with `Sim.Params`, or a library model.
fn semiconductor(
    input: &SimInput,
    fields: &SimFields,
    resolver: &mut Resolver,
) -> Result<Elements, Error> {
    let device = &fields.device;
    match (fields.library, fields.model_name) {
        (Some(library), Some(name)) => {
            if !fields.params.is_empty() {
                return Err(Error::new(
                    "Sim.Params together with Sim.Library is not supported",
                ));
            }
            let bound = library_model(input, library, name, Vec::new(), resolver)?;
            let info = bound.elements[0].info.as_ref().expect("a library model");
            match (&info.model_type, info.sim_device) {
                (None, _) => Err(Error::new(format!(
                    "{name:?} in {library:?} is a subcircuit, not a .model"
                ))),
                (Some(model_type), sim_device) if sim_device != device => Err(Error::new(format!(
                    "model {name:?} has type {model_type:?} (a {sim_device}), which does not match Sim.Device {device}"
                ))),
                _ => Ok(bound),
            }
        }
        (None, None) => {
            let model_type = builtin_model_type(device, fields.sim_type.as_deref())
                .ok_or_else(|| fields.unsupported_type())?;
            let mut definition = model_type.to_string();
            for (key, value) in &fields.params {
                definition.push_str(&format!(" {key}={}", value::param_to_spice(value)));
            }
            let hash = crate::fnv1a(definition.as_bytes());
            let name = format!("{}_{hash:016x}", input.builtin_model_name);
            let text = format!(".model {name} {definition}\n");
            let first = model_type.split(' ').next().unwrap().to_ascii_lowercase();
            let model = model_device(&first, &text).expect("a built-in model type");
            debug_assert_eq!(model.sim_device, device);
            let (nodes, unused) = map_pins(input.symbol, model.terminals)?;
            let models = vec![ModelDef::Inline {
                name: name.clone(),
                text,
            }];
            Ok(element(
                model.letter,
                nodes,
                Args::Fixed(name),
                models,
                unused,
            ))
        }
        _ => Err(Error::new(
            "Sim.Library and Sim.Name must be given together",
        )),
    }
}

/// `Sim.Device SUBCKT`: a library subcircuit with its parameters.
fn subcircuit(
    input: &SimInput,
    fields: &SimFields,
    resolver: &mut Resolver,
) -> Result<Elements, Error> {
    if fields.sim_type.is_some() {
        return Err(fields.unsupported_type());
    }
    let (Some(library), Some(name)) = (fields.library, fields.model_name) else {
        return Err(Error::new(
            "Sim.Device SUBCKT needs Sim.Library and Sim.Name",
        ));
    };
    let bound = library_model(input, library, name, fields.params.clone(), resolver)?;
    if bound.elements[0].device != 'X' {
        return Err(Error::new(format!(
            "{name:?} in {library:?} is a .model, not a subcircuit"
        )));
    }
    Ok(bound)
}

/// No `Sim.*` fields: KiCad infers R, C and L from the reference prefix and the value.
fn inferred(input: &SimInput) -> Result<SpiceDef, Error> {
    let letter = match input
        .ref_prefix
        .chars()
        .next()
        .map(|c| c.to_ascii_uppercase())
    {
        Some(c @ ('R' | 'C' | 'L')) => c,
        _ => return Ok(SpiceDef::Unsupported),
    };
    if input.symbol.pins.len() != 2 {
        return Ok(SpiceDef::Unsupported);
    }
    let args = value_args(input.value, letter, "", "")?;
    let nodes = input
        .symbol
        .pins
        .iter()
        .map(|pin| pin.number.clone())
        .collect();
    Ok(SpiceDef::Elements(element(
        letter,
        nodes,
        args,
        Vec::new(),
        Vec::new(),
    )))
}

/// The arguments for the value of the part (in KiCad's notation, converted to SPICE).
fn value_args(value: &Value, device: char, prefix: &str, suffix: &str) -> Result<Args, Error> {
    match value {
        Value::Param => Ok(Args::Value {
            prefix: prefix.to_string(),
            suffix: suffix.to_string(),
        }),
        Value::Fixed(value) => {
            let spice = value::try_to_spice(value, device)
                .map_err(|e| Error::new(format!("{e}; use `value: param` or a numeric value")))?;
            Ok(Args::Fixed(format!("{prefix}{spice}{suffix}")))
        }
    }
}

fn element(
    device: char,
    nodes: Vec<String>,
    args: Args,
    models: Vec<ModelDef>,
    unused_pins: Vec<String>,
) -> Elements {
    let element = ElementDef {
        device,
        suffix: String::new(),
        nodes,
        args,
        models,
        dialect: Dialect::Ngspice,
        info: None,
    };
    Elements {
        elements: vec![element],
        unused_pins,
    }
}

/// The pins of a symbol, for binding them to a model: the first of duplicate pins only (the others are connected to it).
fn part_pins(symbol: &Symbol) -> Vec<PartPin> {
    symbol
        .distinct_pins()
        .map(|pin| PartPin {
            id: pin.number.clone(),
            name: pin.name.clone(),
        })
        .collect()
}

/// A library model given by `Sim.Library` and `Sim.Name`, connected with `Sim.Pins` like KiCad:
/// pins that are not mapped are unused, and without `Sim.Pins` the pins are taken in order.
fn library_model(
    input: &SimInput,
    library: &str,
    name: &str,
    params: Vec<(String, String)>,
    resolver: &mut Resolver,
) -> Result<Elements, Error> {
    let symbol = input.symbol;
    let pins = parse_sim_pins(symbol.property("Sim.Pins").unwrap_or_default())?;
    let binding = ModelBinding {
        library: library.to_string(),
        name: name.to_string(),
        pins,
        params,
        ..Default::default()
    };
    let part_pins = part_pins(symbol);
    let options = BindOptions {
        pins: PartPins::Given(&part_pins),
        strict: false,
        internal_connections: &[],
        mode: input.model_mode,
    };
    bind(&[binding], &options, resolver).map_err(|e| e.error)
}

/// Parses `Sim.Pins` (`1=K 2=A`).
pub fn parse_sim_pins(text: &str) -> Result<Vec<(String, String)>, Error> {
    text.split_whitespace()
        .map(|pair| {
            pair.split_once('=')
                .map(|(number, pin)| (number.to_string(), pin.to_string()))
                .ok_or_else(|| Error::new(format!("invalid Sim.Pins entry {pair:?}")))
        })
        .collect()
}

/// Writes `key=value` pairs separated by spaces, as in `Sim.Pins` and `Sim.Params`.
pub fn assignments<K: std::fmt::Display, V: std::fmt::Display>(
    pairs: impl IntoIterator<Item = (K, V)>,
) -> String {
    let pairs: Vec<String> = pairs.into_iter().map(|(k, v)| format!("{k}={v}")).collect();
    pairs.join(" ")
}

/// KiCad's model type for a built-in model.
fn builtin_model_type(device: &str, sim_type: Option<&str>) -> Option<&'static str> {
    Some(match (device, sim_type) {
        ("D", None) => "D",
        ("NPN", None | Some("GUMMELPOON")) => "NPN",
        ("PNP", None | Some("GUMMELPOON")) => "PNP",
        ("NMOS", Some("VDMOS")) => "VDMOS NCHAN",
        ("PMOS", Some("VDMOS")) => "VDMOS PCHAN",
        ("NMOS", Some("MOS1")) => "NMOS",
        ("PMOS", Some("MOS1")) => "PMOS",
        ("NJFET", None | Some("SHICHMANHODGES")) => "NJF",
        ("PJFET", None | Some("SHICHMANHODGES")) => "PJF",
        _ => return None,
    })
}

/// Maps the symbol pins to the terminals of a built-in model with `Sim.Pins` (`1=K 2=A`), or in
/// pin order without it, as for library models. Returns the pin numbers in terminal order and the
/// pins not used by the model.
fn map_pins(symbol: &Symbol, terminals: &[&str]) -> Result<(Vec<String>, Vec<String>), Error> {
    let mapping = parse_sim_pins(symbol.property("Sim.Pins").unwrap_or_default())?;
    let terminals: Vec<String> = terminals.iter().map(|t| t.to_string()).collect();
    map_ports(
        &part_pins(symbol),
        &mapping,
        &[],
        &terminals,
        false,
        "the model",
    )
    .map_err(|(_, e)| e)
}

/// Parses `Sim.Params` (`name=value` separated by spaces; values may be double-quoted).
fn parse_params(text: &str) -> Result<Vec<(String, String)>, Error> {
    let mut params = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            return Ok(params);
        }
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            name.push(c);
            chars.next();
        }
        if chars.next() != Some('=') || name.is_empty() {
            return Err(Error::new(format!(
                "invalid Sim.Params {text:?} (expected name=value)"
            )));
        }
        let mut value = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            loop {
                match chars.next() {
                    Some('"') => break,
                    Some(c) => value.push(c),
                    None => {
                        return Err(Error::new(format!(
                            "unterminated quote in Sim.Params {text:?}"
                        )));
                    }
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                value.push(c);
                chars.next();
            }
        }
        params.push((name, value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        symbol::SymbolLib,
        test_util::{fixture_resolver, strings},
    };

    pub(super) fn symbol(fields: &str, pins: &[(&str, &str)]) -> Symbol {
        let pins: String = pins
            .iter()
            .enumerate()
            .map(|(i, (number, name))| {
                format!("(pin passive line (at 0 {i} 0) (name \"{name}\") (number \"{number}\"))")
            })
            .collect();
        let text = format!("(kicad_symbol_lib (symbol \"S\" {fields} (symbol \"S_1_1\" {pins})))");
        SymbolLib::parse(&text).unwrap().symbol("S").unwrap()
    }

    pub(super) fn props(props: &[(&str, &str)]) -> String {
        props
            .iter()
            .map(|(k, v)| format!("(property \"{k}\" \"{}\")", v.replace('"', "\\\"")))
            .collect()
    }

    pub(super) fn convert(
        symbol: &Symbol,
        prefix: &str,
        value: Value,
        resolver: &mut Resolver,
    ) -> Result<SpiceDef, Error> {
        spice_def(
            &SimInput {
                symbol,
                ref_prefix: prefix,
                value: &value,
                builtin_model_name: "__S",
                model_mode: ModelMode::Include,
            },
            resolver,
        )
    }

    /// The element of a part with a single element, and the unused pins.
    fn single(def: SpiceDef) -> (ElementDef, Vec<String>) {
        match def {
            SpiceDef::Elements(Elements {
                mut elements,
                unused_pins,
            }) if elements.len() == 1 => (elements.remove(0), unused_pins),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn test_inferred() {
        let r = symbol("", &[("1", "~"), ("2", "~")]);
        let def = convert(&r, "R", Value::Param, &mut fixture_resolver()).unwrap();
        assert_eq!(
            single(def),
            (
                ElementDef {
                    device: 'R',
                    suffix: String::new(),
                    nodes: strings(&["1", "2"]),
                    args: Args::Value {
                        prefix: "".into(),
                        suffix: "".into(),
                    },
                    models: vec![],
                    dialect: Dialect::Ngspice,
                    info: None,
                },
                vec![]
            )
        );
        assert!(convert(&r, "R", Value::Fixed("R".into()), &mut fixture_resolver()).is_err());
        assert_eq!(
            convert(&r, "U", Value::Param, &mut fixture_resolver()).unwrap(),
            SpiceDef::Unsupported
        );
        let excluded = symbol("(exclude_from_sim yes)", &[("1", "~")]);
        assert_eq!(
            convert(&excluded, "R", Value::Param, &mut fixture_resolver()).unwrap(),
            SpiceDef::Excluded
        );
    }

    #[test]
    fn test_builtin_diode() {
        let d = symbol(
            &props(&[
                ("Sim.Device", "D"),
                ("Sim.Pins", "1=K 2=A"),
                ("Sim.Params", "rs=50m cjo=10p"),
            ]),
            &[("1", "K"), ("2", "A")],
        );
        let (element, _) = single(
            convert(
                &d,
                "D",
                Value::Fixed("1N4148".into()),
                &mut fixture_resolver(),
            )
            .unwrap(),
        );
        assert_eq!(element.device, 'D');
        assert_eq!(element.nodes, strings(&["2", "1"]));
        // The name has the hash of the model, so that parts with different parameters do not
        // share a name.
        let name = format!("__S_{:016x}", crate::fnv1a(b"D rs=50m cjo=10p"));
        assert_eq!(element.args, Args::Fixed(name.clone()));
        assert_eq!(
            element.models,
            vec![ModelDef::Inline {
                text: format!(".model {name} D rs=50m cjo=10p\n"),
                name,
            }]
        );
    }

    #[test]
    fn test_source_and_errors() {
        let v = symbol(
            &props(&[
                ("Sim.Device", "V"),
                ("Sim.Type", "DC"),
                ("Sim.Pins", "1=+ 2=-"),
            ]),
            &[("1", "~"), ("2", "~")],
        );
        let (element, _) =
            single(convert(&v, "V", Value::Fixed("5".into()), &mut fixture_resolver()).unwrap());
        assert_eq!(element.args, Args::Fixed("DC 5".into()));
        let error = |fields: &[(&str, &str)]| {
            let s = symbol(&props(fields), &[("1", "~"), ("2", "~"), ("3", "~")]);
            convert(&s, "Q", Value::Param, &mut fixture_resolver())
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            error(&[("Sim.Device", "SW")]),
            "Sim.Device SW is not supported"
        );
        assert_eq!(
            error(&[("Sim.Device", "NPN"), ("Sim.Type", "VBIC")]),
            "Sim.Type \"VBIC\" of Sim.Device NPN is not supported"
        );
        assert_eq!(
            error(&[("Sim.Device", "NPN"), ("Sim.Pins", "1=C 2=B 3=X")]),
            "the model has no port \"X\"; its ports are C B E"
        );
        let message = error(&[("Sim.Device", "NPN"), ("Sim.Pins", "1=C 2=B")]);
        assert!(
            message.starts_with("no pin is mapped to the ports E of the model\npins and ports:"),
            "{message}"
        );
        assert_eq!(
            error(&[("Sim.Device", "NPN"), ("Sim.Pins", "1=C 4=B 3=E")]),
            "the part has no pin \"4\""
        );
        assert_eq!(
            error(&[("Sim.Device", "D")]),
            "the symbol has 3 pins but the model has 2 (A K); add Sim.Pins"
        );
        assert_eq!(
            error(&[("Sim.Device", "D"), ("Sim.Params", "rs=1;x")]),
            "the value \"1;x\" of rs cannot be written in a deck"
        );
    }

    /// The forms of the sources, as kicad-cli 10.0.8 writes them (except for the PULSE marked).
    #[test]
    fn test_source_waveforms() {
        let args = |device: &str, sim_type: &str, params: &str, value: Value| {
            let s = symbol(
                &props(&[
                    ("Sim.Device", device),
                    ("Sim.Type", sim_type),
                    ("Sim.Params", params),
                ]),
                &[("1", "~"), ("2", "~")],
            );
            convert(&s, "V", value, &mut fixture_resolver()).map(|def| single(def).0.args)
        };
        let fixed = |device: &str, sim_type: &str, params: &str| match args(
            device,
            sim_type,
            params,
            Value::Fixed("2.345".into()),
        )
        .unwrap()
        {
            Args::Fixed(text) => text,
            args => panic!("{args:?}"),
        };
        // The symbols of Simulation_SPICE.
        assert_eq!(
            fixed("V", "SIN", "dc=0 ampl=1 f=1k ac=1"),
            "DC 0 SIN( 0 1 1k 0 0 0 ) AC 1"
        );
        assert_eq!(
            fixed("I", "PULSE", "y1=0 y2=1 td=2n tr=2n tf=2n tw=50n per=100n"),
            "PULSE( 0 1 2n 2n 2n 50n 100n )"
        );
        assert_eq!(
            fixed("V", "EXP", "y1=0 y2=1 td1=2n tau1=30n td2=60n tau2=40n"),
            "EXP( 0 1 2n 30n 60n 40n )"
        );
        assert_eq!(
            fixed(
                "V",
                "PWL",
                r#"pwl="0 -1 50n -1 51n 0 97n 1 171n -1 200n -1""#
            ),
            "PWL( 0 -1 50n -1 51n 0 97n 1 171n -1 200n -1 )"
        );
        assert_eq!(
            fixed(
                "I",
                "AM",
                "vo=1 vmo=2 vma=3 fm=4 fc=5 td=6 phasem=7 phasec=8"
            ),
            "AM( 1 2 3 4 5 6 7 8 )"
        );
        assert_eq!(
            fixed("I", "SFFM", "vo=1 va=2 fm=3 mdi=4 fc=5 phasem=6 phasec=7"),
            "SFFM( 1 2 3 4 5 6 7 )"
        );
        // Parameters before the last one given are 0, the ones after it are left out.
        assert_eq!(fixed("V", "SIN", "AMPL=1 F=1k"), "DC 0 SIN( 0 1 1k )");
        assert_eq!(fixed("V", "EXP", "tau2=3"), "EXP( 0 0 0 0 0 3 )");
        assert_eq!(
            fixed("V", "SIN", "ampl=1 ph=30"),
            "DC 0 SIN( 0 1 0 0 0 0 ) AC 0 30"
        );
        // Numbers in KiCad's notation.
        assert_eq!(fixed("V", "EXP", "y1=1M y2=4k7V"), "EXP( 1Meg 4.7k )");
        assert_eq!(
            fixed("V", "PWL", r#"pwl="0 0 1M 2M""#),
            "PWL( 0 0 1Meg 2Meg )"
        );
        // A single pulse without `per` and `np`. KiCad 10.0.8 appends `0 1` after the parameters
        // given (`PULSE( 0 1 0 1 )`, so `td` 0 and `tr` 1), which is not what it means.
        assert_eq!(fixed("V", "PULSE", "y1=0 y2=1"), "PULSE( 0 1 0 0 0 0 0 1 )");
        assert_eq!(fixed("V", "PULSE", "y2=1 np=3"), "PULSE( 0 1 0 0 0 0 0 3 )");
        // DC: `dc`, or the value, with AC.
        assert_eq!(fixed("V", "DC", "dc=1M ac=2 ph=3"), "DC 1Meg AC 2 3");
        assert_eq!(fixed("I", "", "ph=2"), "DC 2.345 AC 0 2");
        assert_eq!(fixed("I", "DC", ""), "DC 2.345");
        assert_eq!(
            args("V", "DC", "ac=1", Value::Param).unwrap(),
            Args::Value {
                prefix: "DC ".into(),
                suffix: " AC 1".into()
            }
        );

        let error = |device: &str, sim_type: &str, params: &str| {
            args(device, sim_type, params, Value::Param)
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            error("V", "SIN", ""),
            "Sim.Type SIN of Sim.Device V needs Sim.Params"
        );
        assert_eq!(
            error("V", "SIN", "freq=1k"),
            "Sim.Params freq of Sim.Type SIN is not supported (its parameters are dc ampl f td theta phase ac ph)"
        );
        assert_eq!(
            error("V", "SIN", "f=1k f=2k"),
            "Sim.Params f is given twice"
        );
        assert_eq!(
            error("V", "SIN", "f={x}"),
            "Sim.Params f={x} is not a number"
        );
        assert_eq!(
            error("V", "PWL", r#"pwl="0 0 1u""#),
            "Sim.Params pwl=\"0 0 1u\" is not pairs of time and value"
        );
        assert_eq!(
            error("V", "WHITENOISE", "rms=1"),
            "Sim.Type \"WHITENOISE\" of Sim.Device V is not supported"
        );
    }

    #[test]
    fn test_values_in_kicad_notation() {
        let r = symbol("", &[("1", "~"), ("2", "~")]);
        let args = |symbol: &Symbol, value: &str| {
            single(
                convert(
                    symbol,
                    "R",
                    Value::Fixed(value.into()),
                    &mut fixture_resolver(),
                )
                .unwrap(),
            )
            .0
            .args
        };
        // RKM and `M` (mega), which ngspice would read as 4k and milli.
        assert_eq!(args(&r, "4k7"), Args::Fixed("4.7k".into()));
        assert_eq!(args(&r, "1M"), Args::Fixed("1Meg".into()));
        let message = convert(&r, "R", Value::Fixed("R47".into()), &mut fixture_resolver())
            .unwrap_err()
            .to_string();
        assert!(
            message.starts_with("the value \"R47\" is not a number"),
            "{message}"
        );

        let fixed = symbol(
            &props(&[("Sim.Device", "R"), ("Sim.Params", "r=2M2")]),
            &[("1", "~"), ("2", "~")],
        );
        assert_eq!(args(&fixed, "x"), Args::Fixed("2.2Meg".into()));
        let d = symbol(
            &props(&[("Sim.Device", "D"), ("Sim.Params", "bv=1M rs={x}")]),
            &[("1", "A"), ("2", "K")],
        );
        let (element, _) = single(convert(&d, "D", Value::Param, &mut fixture_resolver()).unwrap());
        assert_eq!(
            element.models,
            vec![ModelDef::Inline {
                name: format!("__S_{:016x}", crate::fnv1a(b"D bv=1Meg rs={x}")),
                text: format!(
                    ".model __S_{:016x} D bv=1Meg rs={{x}}\n",
                    crate::fnv1a(b"D bv=1Meg rs={x}")
                ),
            }]
        );
    }

    #[test]
    fn test_library_model_must_match_the_device() {
        let mos = |device: &str, name: &str| {
            let s = symbol(
                &props(&[
                    ("Sim.Device", device),
                    ("Sim.Library", "models/mos.lib"),
                    ("Sim.Name", name),
                ]),
                &[("1", "D"), ("2", "G"), ("3", "S")],
            );
            convert(&s, "Q", Value::Param, &mut fixture_resolver())
        };
        let (element, _) = single(mos("PMOS", "PVD").unwrap());
        assert_eq!(
            (element.device, element.nodes),
            ('M', strings(&["1", "2", "3"]))
        );
        assert!(mos("NMOS", "NVD").is_ok());
        assert_eq!(
            mos("NMOS", "PVD").unwrap_err().to_string(),
            "model \"PVD\" has type \"vdmos\" (a PMOS), which does not match Sim.Device NMOS"
        );
        assert_eq!(
            mos("PMOS", "NVD").unwrap_err().to_string(),
            "model \"NVD\" has type \"vdmos\" (a NMOS), which does not match Sim.Device PMOS"
        );
    }

    #[test]
    fn test_subckt() {
        let amp = symbol(
            &props(&[
                ("Sim.Device", "SUBCKT"),
                ("Sim.Library", "models/amp.lib"),
                ("Sim.Name", "AMP"),
                ("Sim.Pins", "1=out 2=in- 3=in+ 5=vcc 4=vee"),
                ("Sim.Params", "GAIN=100"),
            ]),
            &[
                ("1", "~"),
                ("2", "-"),
                ("3", "+"),
                ("4", "V-"),
                ("5", "V+"),
                ("6", "NC"),
            ],
        );
        let mut r = fixture_resolver();
        let (element, unused_pins) =
            single(convert(&amp, "U", Value::Fixed("AMP".into()), &mut r).unwrap());
        assert_eq!(element.device, 'X');
        assert_eq!(element.nodes, strings(&["3", "2", "5", "4", "1"]));
        assert_eq!(element.args, Args::Fixed("amp GAIN=100".into()));
        assert_eq!(element.dialect, Dialect::Ngspice);
        assert_eq!(
            element.info.as_ref().unwrap().ports,
            strings(&["in+", "in-", "vcc", "vee", "out"])
        );
        assert_eq!(unused_pins, strings(&["6"]));
        let lib = r.manifest_dir.join("models/amp.lib");
        assert_eq!(element.models, vec![ModelDef::Include(lib.clone())]);
        // Included files are read to find definitions, and tracked too.
        assert_eq!(r.files, vec![lib, r.manifest_dir.join("models/clamp.lib")]);

        let mut r = fixture_resolver();
        let (element, _) = single(
            spice_def(
                &SimInput {
                    symbol: &amp,
                    ref_prefix: "U",
                    value: &Value::Param,
                    builtin_model_name: "__S",
                    model_mode: ModelMode::Inline,
                },
                &mut r,
            )
            .unwrap(),
        );
        let names: Vec<_> = element
            .models
            .iter()
            .map(|m| match m {
                ModelDef::Inline { name, .. } => name.as_str(),
                _ => panic!(),
            })
            .collect();
        // `clamp` is used by `amp` and defined in the included file.
        assert_eq!(names, ["amp", "clamp"]);
        assert_eq!(r.files.len(), 2);
    }

    #[test]
    fn test_parse_params() {
        assert_eq!(
            parse_params(" a=1  b=\"2k\" ").unwrap(),
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2k".to_string())
            ]
        );
        assert!(parse_params("a").is_err());
    }
}
