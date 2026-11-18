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

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    Error,
    resolve::Resolver,
    spice_lib::{Definition, DefinitionKind, SpiceLib},
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
    Element(ElementDef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementDef {
    pub device: char,
    /// Pin numbers in the order of the model's nodes.
    pub nodes: Vec<String>,
    pub args: Args,
    pub models: Vec<ModelDef>,
    /// Pins not connected to the model.
    pub unused_pins: Vec<String>,
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
                    let (definition, models) = library_definition(library, name, input.model_mode, resolver)?;
                    let DefinitionKind::Model { model_type } = &definition.kind else {
                        return Err(Error::new(format!("{name:?} in {library:?} is a subcircuit, not a .model")));
                    };
                    let letter = device_letter(&device, model_type).ok_or_else(|| {
                        Error::new(format!(
                            "model {name:?} has type {model_type:?}, which does not match Sim.Device {device}"
                        ))
                    })?;
                    (letter, (name.to_string(), model_type.clone(), models))
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
            let (definition, models) = library_definition(library, name, input.model_mode, resolver)?;
            let DefinitionKind::Subckt { ports, .. } = &definition.kind else {
                return Err(Error::new(format!("{name:?} in {library:?} is a .model, not a subcircuit")));
            };
            let ports: Vec<&str> = ports.iter().map(String::as_str).collect();
            let nodes = map_pins(symbol, &ports).map_err(|e| e.context(format!("subcircuit {name}")))?;
            let mut args = definition.name.clone();
            for (key, value) in &params {
                args.push_str(&format!(" {key}={value}"));
            }
            Ok(element('X', nodes, Args::Fixed(args), models))
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
    Ok(SpiceDef::Element(ElementDef {
        device: letter,
        nodes,
        args,
        models: Vec::new(),
        unused_pins: Vec::new(),
    }))
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
    SpiceDef::Element(ElementDef {
        device,
        nodes,
        args,
        models,
        unused_pins,
    })
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
            for pair in mapping.split_whitespace() {
                let (number, model_pin) = pair
                    .split_once('=')
                    .ok_or_else(|| Error::new(format!("invalid Sim.Pins entry {pair:?}")))?;
                if !symbol.pins.iter().any(|pin| pin.number == number) {
                    return Err(Error::new(format!("Sim.Pins refers to pin {number:?}, which the symbol does not have")));
                }
                let index = model_pins
                    .iter()
                    .position(|p| p.eq_ignore_ascii_case(model_pin))
                    .ok_or_else(|| {
                        Error::new(format!(
                            "Sim.Pins refers to model pin {model_pin:?}; the model has {}",
                            model_pins.join(" ")
                        ))
                    })?;
                if nodes[index].replace(number.to_string()).is_some() {
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

/// Names that a definition may refer to: the words of its text after the keyword and its own name.
/// Names may contain `+` and `-` (`IN+`), which are also operators, so both forms are returned.
fn referenced_names(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('*'))
        .flat_map(|line| line.split(|c: char| c.is_whitespace() || "(),={}'\"*/<>&|?:!^;".contains(c)))
        .filter(|word| !word.is_empty())
        .skip(2)
        .flat_map(|word| std::iter::once(word).chain(word.split(['+', '-']).filter(|w| !w.is_empty())))
}

/// A library file with the section that is read (`None`: outside the sections).
type LibSection = (Arc<SpiceLib>, Option<String>);

/// Reads a library and the files it includes, each with the section that is read.
fn read_library(path: &Path, resolver: &mut Resolver) -> Result<Vec<LibSection>, Error> {
    let mut libs = Vec::new();
    let mut queue = vec![(path.to_path_buf(), None::<String>)];
    let mut seen = BTreeSet::new();
    while let Some((file, section)) = queue.pop() {
        let key = (file.clone(), section.as_ref().map(|s| s.to_ascii_lowercase()));
        if !seen.insert(key) || seen.len() > 32 {
            continue;
        }
        let lib = resolver.spice_lib(&file)?;
        if let Some(section) = &section
            && !lib.sections.iter().any(|s| s.eq_ignore_ascii_case(section))
        {
            return Err(Error::new(format!("{} has no section {section:?}", file.display())));
        }
        let dir = file.parent().map(PathBuf::from).unwrap_or_default();
        // Relative paths are relative to the including file, as in ngspice.
        queue.extend(
            lib.includes(section.as_deref())
                .map(|include| (dir.join(&include.path), include.section.clone())),
        );
        libs.push((lib, section));
    }
    Ok(libs)
}

/// Finds the definition `name` in a library (following its `.include` lines) and returns it
/// with the model sources the element needs.
fn library_definition(
    library: &str,
    name: &str,
    mode: ModelMode,
    resolver: &mut Resolver,
) -> Result<(Definition, Vec<ModelDef>), Error> {
    let path = resolver.sim_library_path(library)?;
    let libs = read_library(&path, resolver)?;
    let find = |name: &str| {
        libs.iter()
            .find_map(|(lib, section)| lib.find_in(name, section.as_deref()))
            .cloned()
    };
    let definition = find(name).ok_or_else(|| {
        Error::new(format!("no .subckt or .model named {name:?} in {}", path.display()))
    })?;
    let models = match mode {
        ModelMode::Include => vec![ModelDef::Include(path)],
        ModelMode::Inline => {
            // The definition and the top-level definitions (also `.param` / `.func`) it refers to by name.
            let find_any = |name: &str| {
                libs.iter()
                    .find_map(|(lib, section)| {
                        lib.definitions(section.as_deref())
                            .find(|d| d.name.eq_ignore_ascii_case(name))
                    })
                    .cloned()
            };
            let mut out: Vec<Definition> = Vec::new();
            let mut pending = vec![definition.clone()];
            while let Some(def) = pending.pop() {
                if out.iter().any(|d| d.name.eq_ignore_ascii_case(&def.name)) {
                    continue;
                }
                for token in referenced_names(&def.text) {
                    if let Some(referenced) = find_any(token) {
                        pending.push(referenced);
                    }
                }
                out.push(def);
            }
            out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
            out.into_iter()
                .map(|d| ModelDef::Inline { name: d.name, text: d.text })
                .collect()
        }
    };
    Ok((definition, models))
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

    #[test]
    fn test_inferred() {
        let r = symbol("", &[("1", "~"), ("2", "~")]);
        let def = convert(&r, "R", Value::Param, &mut resolver()).unwrap();
        assert_eq!(
            def,
            SpiceDef::Element(ElementDef {
                device: 'R',
                nodes: strings(&["1", "2"]),
                args: Args::Value { prefix: "".into() },
                models: vec![],
                unused_pins: vec![],
            })
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
        let SpiceDef::Element(element) = convert(&d, "D", Value::Fixed("1N4148".into()), &mut resolver()).unwrap()
        else {
            panic!()
        };
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
        let SpiceDef::Element(element) = convert(&v, "V", Value::Fixed("5".into()), &mut resolver()).unwrap() else {
            panic!()
        };
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
        let SpiceDef::Element(element) = convert(&amp, "U", Value::Fixed("AMP".into()), &mut r).unwrap() else {
            panic!()
        };
        assert_eq!(element.device, 'X');
        assert_eq!(element.nodes, strings(&["3", "2", "5", "4", "1"]));
        assert_eq!(element.args, Args::Fixed("amp GAIN=100".into()));
        assert_eq!(element.unused_pins, strings(&["6"]));
        let lib = r.manifest_dir.join("models/amp.lib");
        assert_eq!(element.models, vec![ModelDef::Include(lib.clone())]);
        // Included files are read to find definitions, and tracked too.
        assert_eq!(r.files, vec![lib, r.manifest_dir.join("models/clamp.lib")]);

        let mut r = resolver();
        let SpiceDef::Element(element) = spice_def(
            &SimInput {
                symbol: &amp,
                ref_prefix: "U",
                value: &Value::Param,
                builtin_model_name: "__S",
                model_mode: ModelMode::Inline,
            },
            &mut r,
        )
        .unwrap() else {
            panic!()
        };
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
