//! Turning the `Sim.*` fields of a symbol into the SPICE description of a part.
//!
//! The supported subset of KiCad 9's simulation models:
//! - no `Sim.*` fields: R, C and L inferred from the reference prefix, with the value as is;
//!   other symbols are not simulated (unsupported, not an error);
//! - `exclude_from_sim` / `Sim.Enable 0`: excluded from the simulation;
//! - `Sim.Device` `R` `C` `L`, `V` `I` (`Sim.Type DC`), `D`, `NPN` `PNP` (Gummel-Poon),
//!   `NMOS` `PMOS` (`VDMOS`, `MOS1`), `NJFET` `PJFET` (Shichman-Hodges), with built-in model
//!   parameters (`Sim.Params`) or a library model (`Sim.Library` + `Sim.Name`);
//! - `Sim.Device SUBCKT` with `Sim.Library` + `Sim.Name` (+ `Sim.Params`).
//!
//! Anything else is an error: the macro user decides with `spice: none` / `spice: excluded`.

use std::{collections::BTreeSet, path::PathBuf};

use crate::{
    Error,
    binding::{BindOptions, ModelBinding, ModelInfo, PartPin, bind},
    resolve::Resolver,
    spice_lib::Dialect,
    symbol::Symbol,
};

/// The value of a part: fixed at compile time or given when the part is created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Fixed(String),
    Param,
}

/// The SPICE description of a part, with pins identified by their numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpiceDef {
    /// The symbol has no simulation model.
    Unsupported,
    Excluded,
    Elements {
        elements: Vec<ElementDef>,
        /// Pins not connected to the model.
        unused_pins: Vec<String>,
    },
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

/// The arguments after the nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Args {
    Fixed(String),
    /// `prefix` followed by the value given when the part is created.
    Value { prefix: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelDef {
    /// An absolute path to include.
    Include(PathBuf),
    Inline { name: String, text: String },
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
    /// Name for a built-in model (`.model <name> D ...`), unique to the generated part type.
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
    let device = device.to_ascii_uppercase();
    let sim_type = symbol
        .property("Sim.Type")
        .filter(|t| !t.is_empty())
        .map(str::to_ascii_uppercase);
    let params = parse_params(symbol.property("Sim.Params").unwrap_or_default())?;
    let library = symbol.property("Sim.Library").filter(|l| !l.is_empty());
    let model_name = symbol.property("Sim.Name").filter(|n| !n.is_empty());
    let unsupported_type = || {
        Error::new(format!(
            "Sim.Type {:?} of Sim.Device {device} is not supported",
            sim_type.as_deref().unwrap_or_default()
        ))
    };

    match device.as_str() {
        "R" | "C" | "L" | "V" | "I" => {
            let letter = device.chars().next().unwrap();
            let source = matches!(letter, 'V' | 'I');
            if library.is_some() {
                return Err(Error::new(format!("Sim.Library for Sim.Device {device} is not supported")));
            }
            if source && sim_type.as_deref().is_some_and(|t| t != "DC") {
                return Err(unsupported_type());
            }
            if !source && sim_type.is_some() {
                return Err(unsupported_type());
            }
            let key = if source { "dc" } else { &device.to_ascii_lowercase() };
            let prefix = if source { "DC " } else { "" };
            let args = match params.as_slice() {
                [] => value_args(input.value, prefix)?,
                [(name, value)] if name.eq_ignore_ascii_case(key) => Args::Fixed(format!("{prefix}{value}")),
                _ => {
                    return Err(Error::new(format!(
                        "Sim.Params {:?} of Sim.Device {device} is not supported (only {key}=...)",
                        symbol.property("Sim.Params").unwrap_or_default()
                    )));
                }
            };
            let nodes = map_pins(symbol, &["+", "-"])?;
            Ok(element(letter, nodes, args, Vec::new()))
        }
        "D" | "NPN" | "PNP" | "NMOS" | "PMOS" | "NJFET" | "PJFET" => {
            let (letter, model) = match (library, model_name) {
                (Some(library), Some(name)) => {
                    if !params.is_empty() {
                        return Err(Error::new("Sim.Params together with Sim.Library is not supported"));
                    }
                    let def = library_model(input, library, name, Vec::new(), resolver)?;
                    let SpiceDef::Elements { elements, .. } = &def else { unreachable!() };
                    let Some(model_type) = elements[0].info.as_ref().and_then(|info| info.model_type.as_ref()) else {
                        return Err(Error::new(format!("{name:?} in {library:?} is a subcircuit, not a .model")));
                    };
                    if device_letter(&device, model_type).is_none() {
                        return Err(Error::new(format!(
                            "model {name:?} has type {model_type:?}, which does not match Sim.Device {device}"
                        )));
                    }
                    return Ok(def);
                }
                (None, None) => {
                    let model_type = builtin_model_type(&device, sim_type.as_deref()).ok_or_else(unsupported_type)?;
                    let letter = device_letter(&device, &model_type.to_ascii_lowercase()).unwrap();
                    let name = input.builtin_model_name.to_string();
                    let mut text = format!(".model {name} {model_type}");
                    for (key, value) in &params {
                        text.push_str(&format!(" {key}={value}"));
                    }
                    text.push('\n');
                    let models = vec![ModelDef::Inline { name: name.clone(), text }];
                    (letter, (name, model_type.to_ascii_lowercase(), models))
                }
                _ => return Err(Error::new("Sim.Library and Sim.Name must be given together")),
            };
            let (name, model_type, models) = model;
            let model_pins: &[&str] = match letter {
                'D' => &["A", "K"],
                'Q' => &["C", "B", "E"],
                'M' if model_type.starts_with("vdmos") => &["D", "G", "S"],
                'M' => &["D", "G", "S", "B"],
                'J' => &["D", "G", "S"],
                _ => unreachable!(),
            };
            let nodes = map_pins(symbol, model_pins)?;
            Ok(element(letter, nodes, Args::Fixed(name), models))
        }
        "SUBCKT" => {
            if sim_type.is_some() {
                return Err(unsupported_type());
            }
            let (Some(library), Some(name)) = (library, model_name) else {
                return Err(Error::new("Sim.Device SUBCKT needs Sim.Library and Sim.Name"));
            };
            let def = library_model(input, library, name, params, resolver)?;
            let SpiceDef::Elements { elements, .. } = &def else { unreachable!() };
            if elements[0].device != 'X' {
                return Err(Error::new(format!("{name:?} in {library:?} is a .model, not a subcircuit")));
            }
            Ok(def)
        }
        _ => Err(Error::new(format!("Sim.Device {device} is not supported"))),
    }
}

/// No `Sim.*` fields: KiCad infers R, C and L from the reference prefix and the value.
fn inferred(input: &SimInput) -> Result<SpiceDef, Error> {
    let letter = match input.ref_prefix.chars().next().map(|c| c.to_ascii_uppercase()) {
        Some(c @ ('R' | 'C' | 'L')) => c,
        _ => return Ok(SpiceDef::Unsupported),
    };
    if input.symbol.pins.len() != 2 {
        return Ok(SpiceDef::Unsupported);
    }
    let args = value_args(input.value, "")?;
    let nodes = input.symbol.pins.iter().map(|pin| pin.number.clone()).collect();
    Ok(element(letter, (nodes, Vec::new()), args, Vec::new()))
}

fn value_args(value: &Value, prefix: &str) -> Result<Args, Error> {
    match value {
        Value::Param => Ok(Args::Value {
            prefix: prefix.to_string(),
        }),
        Value::Fixed(value) if value.starts_with(|c: char| c.is_ascii_digit() || c == '.' || c == '-') => {
            Ok(Args::Fixed(format!("{prefix}{value}")))
        }
        Value::Fixed(value) => Err(Error::new(format!(
            "the value {value:?} is not a number; use `value: param` or a numeric value"
        ))),
    }
}

fn element(device: char, (nodes, unused_pins): (Vec<String>, Vec<String>), args: Args, models: Vec<ModelDef>) -> SpiceDef {
    SpiceDef::Elements {
        elements: vec![ElementDef {
            device,
            suffix: String::new(),
            nodes,
            args,
            models,
            dialect: Dialect::Ngspice,
            info: None,
        }],
        unused_pins,
    }
}

/// A library model given by `Sim.Library` and `Sim.Name`, connected with `Sim.Pins` like KiCad:
/// pins that are not mapped are unused, and without `Sim.Pins` the pins are taken in order.
fn library_model(
    input: &SimInput,
    library: &str,
    name: &str,
    params: Vec<(String, String)>,
    resolver: &mut Resolver,
) -> Result<SpiceDef, Error> {
    let symbol = input.symbol;
    let pins = parse_sim_pins(symbol.property("Sim.Pins").unwrap_or_default())?;
    let binding = ModelBinding {
        library: library.to_string(),
        name: name.to_string(),
        pins,
        params,
        ..Default::default()
    };
    let part_pins: Vec<PartPin> = symbol
        .pins
        .iter()
        .map(|pin| PartPin {
            id: pin.number.clone(),
            name: pin.name.clone(),
        })
        .collect();
    let options = BindOptions {
        pins: &part_pins,
        strict: false,
        internal_connections: &[],
        mode: input.model_mode,
    };
    let (elements, unused_pins) = bind(&[binding], &options, resolver).map_err(|e| e.error)?;
    Ok(SpiceDef::Elements { elements, unused_pins })
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

/// The element letter for a device whose model has `model_type` (lowercase), if they match.
fn device_letter(device: &str, model_type: &str) -> Option<char> {
    let first = model_type.split_whitespace().next().unwrap_or_default();
    match (device, first) {
        ("D", "d") => Some('D'),
        ("NPN", "npn") | ("PNP", "pnp") => Some('Q'),
        ("NMOS", "nmos" | "vdmos") | ("PMOS", "pmos" | "vdmos") => Some('M'),
        ("NJFET", "njf") | ("PJFET", "pjf") => Some('J'),
        _ => None,
    }
}

/// Maps the symbol pins to the model pins with `Sim.Pins` (`1=K 2=A`), or in pin order without
/// it. Returns the pin numbers in model order and the pins not used by the model.
fn map_pins(symbol: &Symbol, model_pins: &[&str]) -> Result<(Vec<String>, Vec<String>), Error> {
    let mut nodes: Vec<Option<String>> = vec![None; model_pins.len()];
    match symbol.property("Sim.Pins").filter(|p| !p.trim().is_empty()) {
        None => {
            if symbol.pins.len() != model_pins.len() {
                return Err(Error::new(format!(
                    "the symbol has {} pins but the model has {} ({}); add Sim.Pins",
                    symbol.pins.len(),
                    model_pins.len(),
                    model_pins.join(" ")
                )));
            }
            for (node, pin) in nodes.iter_mut().zip(&symbol.pins) {
                *node = Some(pin.number.clone());
            }
        }
        Some(mapping) => {
            for (number, model_pin) in parse_sim_pins(mapping)? {
                if !symbol.pins.iter().any(|pin| pin.number == number) {
                    return Err(Error::new(format!("Sim.Pins refers to pin {number:?}, which the symbol does not have")));
                }
                let index = model_pins
                    .iter()
                    .position(|p| p.eq_ignore_ascii_case(&model_pin))
                    .ok_or_else(|| {
                        Error::new(format!(
                            "Sim.Pins refers to model pin {model_pin:?}; the model has {}",
                            model_pins.join(" ")
                        ))
                    })?;
                if nodes[index].replace(number).is_some() {
                    return Err(Error::new(format!("Sim.Pins maps model pin {model_pin:?} twice")));
                }
            }
        }
    }
    let missing: Vec<&str> = model_pins
        .iter()
        .zip(&nodes)
        .filter(|(_, node)| node.is_none())
        .map(|(pin, _)| *pin)
        .collect();
    if !missing.is_empty() {
        return Err(Error::new(format!("Sim.Pins does not map model pins {}", missing.join(" "))));
    }
    let nodes: Vec<String> = nodes.into_iter().map(Option::unwrap).collect();
    let used: BTreeSet<&String> = nodes.iter().collect();
    if used.len() != nodes.len() {
        return Err(Error::new("Sim.Pins maps a symbol pin to several model pins"));
    }
    let unused = symbol
        .pins
        .iter()
        .filter(|pin| !used.contains(&pin.number))
        .map(|pin| pin.number.clone())
        .collect();
    Ok((nodes, unused))
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
            return Err(Error::new(format!("invalid Sim.Params {text:?} (expected name=value)")));
        }
        let mut value = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            loop {
                match chars.next() {
                    Some('"') => break,
                    Some(c) => value.push(c),
                    None => return Err(Error::new(format!("unterminated quote in Sim.Params {text:?}"))),
                }
            }
            if value.chars().any(char::is_whitespace) {
                return Err(Error::new(format!("Sim.Params value {value:?} with spaces is not supported")));
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
        if value.is_empty() || value.contains(['\\', '\'', '{', '}', ';', '$']) {
            return Err(Error::new(format!("Sim.Params value {value:?} of {name} is not supported")));
        }
        params.push((name, value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol::SymbolLib;

    fn symbol(fields: &str, pins: &[(&str, &str)]) -> Symbol {
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

    fn props(props: &[(&str, &str)]) -> String {
        props
            .iter()
            .map(|(k, v)| format!("(property \"{k}\" \"{v}\")"))
            .collect()
    }

    fn convert(symbol: &Symbol, prefix: &str, value: Value, resolver: &mut Resolver) -> Result<SpiceDef, Error> {
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

    fn resolver() -> Resolver {
        Resolver::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"), |_| None)
    }

    fn strings(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    /// The element of a part with a single element, and the unused pins.
    fn single(def: SpiceDef) -> (ElementDef, Vec<String>) {
        match def {
            SpiceDef::Elements { mut elements, unused_pins } if elements.len() == 1 => (elements.remove(0), unused_pins),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn test_inferred() {
        let r = symbol("", &[("1", "~"), ("2", "~")]);
        let def = convert(&r, "R", Value::Param, &mut resolver()).unwrap();
        assert_eq!(
            single(def),
            (
                ElementDef {
                    device: 'R',
                    suffix: String::new(),
                    nodes: strings(&["1", "2"]),
                    args: Args::Value { prefix: "".into() },
                    models: vec![],
                    dialect: Dialect::Ngspice,
                    info: None,
                },
                vec![]
            )
        );
        assert!(convert(&r, "R", Value::Fixed("R".into()), &mut resolver()).is_err());
        assert_eq!(convert(&r, "U", Value::Param, &mut resolver()).unwrap(), SpiceDef::Unsupported);
        let excluded = symbol("(exclude_from_sim yes)", &[("1", "~")]);
        assert_eq!(convert(&excluded, "R", Value::Param, &mut resolver()).unwrap(), SpiceDef::Excluded);
    }

    #[test]
    fn test_builtin_diode() {
        let d = symbol(
            &props(&[("Sim.Device", "D"), ("Sim.Pins", "1=K 2=A"), ("Sim.Params", "rs=50m cjo=10p")]),
            &[("1", "K"), ("2", "A")],
        );
        let (element, _) = single(convert(&d, "D", Value::Fixed("1N4148".into()), &mut resolver()).unwrap());
        assert_eq!(element.device, 'D');
        assert_eq!(element.nodes, strings(&["2", "1"]));
        assert_eq!(element.args, Args::Fixed("__S".into()));
        assert_eq!(
            element.models,
            vec![ModelDef::Inline {
                name: "__S".into(),
                text: ".model __S D rs=50m cjo=10p\n".into()
            }]
        );
    }

    #[test]
    fn test_source_and_errors() {
        let v = symbol(
            &props(&[("Sim.Device", "V"), ("Sim.Type", "DC"), ("Sim.Pins", "1=+ 2=-")]),
            &[("1", "~"), ("2", "~")],
        );
        let (element, _) = single(convert(&v, "V", Value::Fixed("5".into()), &mut resolver()).unwrap());
        assert_eq!(element.args, Args::Fixed("DC 5".into()));
        let error = |fields: &[(&str, &str)]| {
            let s = symbol(&props(fields), &[("1", "~"), ("2", "~"), ("3", "~")]);
            convert(&s, "Q", Value::Param, &mut resolver()).unwrap_err().to_string()
        };
        assert_eq!(error(&[("Sim.Device", "SW")]), "Sim.Device SW is not supported");
        assert_eq!(
            error(&[("Sim.Device", "NPN"), ("Sim.Type", "VBIC")]),
            "Sim.Type \"VBIC\" of Sim.Device NPN is not supported"
        );
        assert_eq!(
            error(&[("Sim.Device", "NPN"), ("Sim.Pins", "1=C 2=B 3=X")]),
            "Sim.Pins refers to model pin \"X\"; the model has C B E"
        );
        assert_eq!(
            error(&[("Sim.Device", "NPN"), ("Sim.Pins", "1=C 2=B")]),
            "Sim.Pins does not map model pins E"
        );
        assert_eq!(
            error(&[("Sim.Device", "D")]),
            "the symbol has 3 pins but the model has 2 (A K); add Sim.Pins"
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
            &[("1", "~"), ("2", "-"), ("3", "+"), ("4", "V-"), ("5", "V+"), ("6", "NC")],
        );
        let mut r = resolver();
        let (element, unused_pins) = single(convert(&amp, "U", Value::Fixed("AMP".into()), &mut r).unwrap());
        assert_eq!(element.device, 'X');
        assert_eq!(element.nodes, strings(&["3", "2", "5", "4", "1"]));
        assert_eq!(element.args, Args::Fixed("amp GAIN=100".into()));
        assert_eq!(element.dialect, Dialect::Ngspice);
        assert_eq!(element.info.as_ref().unwrap().ports, strings(&["in+", "in-", "vcc", "vee", "out"]));
        assert_eq!(unused_pins, strings(&["6"]));
        let lib = r.manifest_dir.join("models/amp.lib");
        assert_eq!(element.models, vec![ModelDef::Include(lib.clone())]);
        // Included files are read to find definitions, and tracked too.
        assert_eq!(r.files, vec![lib, r.manifest_dir.join("models/clamp.lib")]);

        let mut r = resolver();
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
            vec![("a".to_string(), "1".to_string()), ("b".to_string(), "2k".to_string())]
        );
        assert!(parse_params("type=\\").is_err());
        assert!(parse_params("a").is_err());
    }
}
