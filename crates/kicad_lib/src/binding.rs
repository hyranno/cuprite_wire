//! Binding a model of a SPICE library (a `.subckt` or a `.model`) to the pins of a part.
//!
//! The three ways a part gets a library model share this: a symbol's `Sim.*` fields, the `sim`
//! block of `kicad_part!`, and `spice_part!`. Pins are mapped to ports by name; the order of the
//! nodes is always the order of the ports in the `.subckt` line (or of the device's terminals).
//! The bodies of the models are not interpreted.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    Error,
    resolve::Resolver,
    sim::{Args, ElementDef, ModelDef, ModelMode},
    spice_lib::{Definition, DefinitionKind, Dialect, DialectHint, SpiceLib},
};

/// The dialect of a model: detected from the library, or given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DialectOption {
    #[default]
    Auto,
    Given(Dialect),
}

/// A library model and how the pins of a part are connected to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelBinding {
    /// `${VAR}/...` or a path relative to the base of model libraries.
    pub library: String,
    /// The name of the `.subckt` or `.model`.
    pub name: String,
    /// Pin of the part (a pin number for KiCad parts) => port of the `.subckt`, or terminal of the
    /// device for a `.model` (`A` `K`, `C` `B` `E`, `D` `G` `S` [`B`]).
    pub pins: Vec<(String, String)>,
    /// Pins not connected to the model.
    pub unused: Vec<String>,
    /// Parameters of the `.subckt`.
    pub params: Vec<(String, String)>,
    pub dialect: DialectOption,
    /// Appended to the element name when a part has several elements (`XU1A`).
    pub suffix: String,
}

/// The option of a [`ModelBinding`] an error is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingField {
    Library,
    Name,
    Pins,
    Unused,
    Params,
    Dialect,
    Suffix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingError {
    /// The index of the binding.
    pub element: usize,
    pub field: BindingField,
    pub error: Error,
}

/// A pin of the part, as shown in error messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartPin {
    pub id: String,
    pub name: Option<String>,
}

/// The pins of the part being bound.
#[derive(Debug, Clone, Copy)]
pub enum PartPins<'a> {
    Given(&'a [PartPin]),
    /// The part is made from the model: its pins are the ports (`spice_part!`).
    Ports,
}

pub struct BindOptions<'a> {
    pub pins: PartPins<'a>,
    /// Every pin must be mapped, declared unused, or connected inside the part to a mapped pin.
    /// Otherwise (KiCad's `Sim.Pins`), pins that are not mapped are unused, and without any mapping
    /// the pins are connected to the ports in order.
    pub strict: bool,
    /// Groups of pins connected inside the part.
    pub internal_connections: &'a [Vec<String>],
    pub mode: ModelMode,
}

/// What a part's documentation shows about its model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// The library file, absolute.
    pub path: PathBuf,
    /// The name as defined in the library.
    pub name: String,
    /// The type of a `.model` (lowercase); `None` for a `.subckt`.
    pub model_type: Option<String>,
    /// The ports in node order.
    pub ports: Vec<String>,
    /// The parameters of the `.subckt` with their defaults.
    pub params: Vec<(String, String)>,
    pub dialect: Dialect,
    /// Why the dialect was chosen.
    pub dialect_reason: String,
}

/// The elements of a part, with pins in node order, and the pins not connected to any element.
pub type Bound = (Vec<ElementDef>, Vec<String>);

pub fn bind(
    bindings: &[ModelBinding],
    options: &BindOptions,
    resolver: &mut Resolver,
) -> Result<Bound, BindingError> {
    let mut elements = Vec::new();
    let mut unused: Vec<String> = Vec::new();
    for (index, binding) in bindings.iter().enumerate() {
        let at = |field| {
            move |error| BindingError {
                element: index,
                field,
                error,
            }
        };
        let (element, unmapped) =
            bind_one(binding, options, resolver).map_err(|(field, e)| at(field)(e))?;
        for pin in binding.unused.iter().chain(&unmapped) {
            if !unused.contains(pin) {
                unused.push(pin.clone());
            }
        }
        elements.push(element);
    }
    let mapped: BTreeSet<&String> = elements.iter().flat_map(|e| &e.nodes).collect();
    for (index, binding) in bindings.iter().enumerate() {
        if let Some(pin) = binding.unused.iter().find(|pin| mapped.contains(pin)) {
            return Err(BindingError {
                element: index,
                field: BindingField::Unused,
                error: Error::new(format!(
                    "pin {pin:?} is mapped to a port, so it cannot be unused"
                )),
            });
        }
    }
    let mut suffixes = BTreeSet::new();
    for (index, binding) in bindings.iter().enumerate() {
        let suffix = &binding.suffix;
        let error = |message: String| BindingError {
            element: index,
            field: BindingField::Suffix,
            error: Error::new(message),
        };
        if !suffix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(error(format!(
                "suffix {suffix:?} may only contain ASCII letters, digits and '_'"
            )));
        }
        if !suffixes.insert(suffix.to_ascii_uppercase()) {
            return Err(error(if bindings.len() > 1 && suffix.is_empty() {
                "give each model a distinct `suffix` (e.g. \"A\", \"B\")".into()
            } else {
                format!("suffix {suffix:?} is used twice")
            }));
        }
    }
    if let (true, PartPins::Given(pins)) = (options.strict, options.pins) {
        let covered = |pin: &String| {
            mapped.contains(pin)
                || unused.contains(pin)
                || options.internal_connections.iter().any(|group| {
                    group.contains(pin) && group.iter().any(|other| mapped.contains(other))
                })
        };
        let missing: Vec<&PartPin> = pins.iter().filter(|pin| !covered(&pin.id)).collect();
        if !missing.is_empty() {
            let list: Vec<String> = missing.iter().map(|pin| describe_pin(pin)).collect();
            let ids: Vec<String> = missing.iter().map(|pin| format!("{:?}", pin.id)).collect();
            let mut message = format!(
                "pins not mapped to the model: {}; map them in `pins`, or declare them with `unused: [{}]`",
                list.join(", "),
                ids.join(", ")
            );
            message.push_str(&pin_table(pins, &elements));
            return Err(BindingError {
                element: 0,
                field: BindingField::Pins,
                error: Error::new(message),
            });
        }
    }
    Ok((elements, unused))
}

type FieldError = (BindingField, Error);

fn bind_one(
    binding: &ModelBinding,
    options: &BindOptions,
    resolver: &mut Resolver,
) -> Result<(ElementDef, Vec<String>), FieldError> {
    use BindingField::{Library, Name, Params, Pins, Unused};
    let fail = |field, message: String| (field, Error::new(message));
    let path = resolver
        .sim_library_path(&binding.library)
        .map_err(|e| (Library, e))?;
    let libs = read_library(&path, resolver).map_err(|e| (Library, e))?;
    let found = libs
        .iter()
        .find_map(|(lib, section, _)| lib.find_in(&binding.name, section.as_deref()))
        .cloned()
        .ok_or_else(|| (Name, not_found(&binding.name, &path, &libs)))?;
    let what = match &found.kind {
        DefinitionKind::Model { .. } => format!("model {}", found.name),
        _ => format!("subcircuit {}", found.name),
    };

    // The device and its ports.
    let (device, ports, model_type, defaults) = match &found.kind {
        DefinitionKind::Subckt {
            ports,
            optional_ports,
            params,
        } => {
            if !optional_ports.is_empty() {
                let names: Vec<&str> = optional_ports
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect();
                return Err(fail(
                    Name,
                    format!(
                        "{what} has `OPTIONAL:` ports ({}), which ngspice does not support",
                        names.join(", ")
                    ),
                ));
            }
            ('X', ports.clone(), None, params.clone())
        }
        DefinitionKind::Model { model_type } => {
            let (device, terminals) = model_device(model_type).ok_or_else(|| {
                fail(
                    Name,
                    format!(
                        "{what} has the type {model_type:?}; supported types are D, NPN, PNP, NMOS, PMOS, VDMOS, NJF and PJF"
                    ),
                )
            })?;
            (
                device,
                terminals.iter().map(|t| t.to_string()).collect(),
                Some(model_type.clone()),
                Vec::new(),
            )
        }
        _ => unreachable!("find_in returns subcircuits and models"),
    };
    let dependencies = dependencies(&found, &libs);
    if let Some(locked) = dependencies.iter().find(|d| d.encrypted) {
        let detail = if locked.name.eq_ignore_ascii_case(&found.name) {
            String::new()
        } else {
            format!(" (used by {})", found.name)
        };
        return Err(fail(
            Name,
            format!(
                "{} {}{detail} is encrypted, which ngspice cannot read; ask the vendor for an unencrypted model",
                if matches!(locked.kind, DefinitionKind::Model { .. }) {
                    "model"
                } else {
                    "subcircuit"
                },
                locked.name
            ),
        ));
    }

    // Pins to ports.
    let (part_pins, mapping): (Vec<PartPin>, Vec<(String, String)>) = match options.pins {
        PartPins::Given(pins) => (pins.to_vec(), binding.pins.clone()),
        PartPins::Ports => (
            ports
                .iter()
                .map(|port| PartPin {
                    id: port.clone(),
                    name: None,
                })
                .collect(),
            ports
                .iter()
                .map(|port| (port.clone(), port.clone()))
                .collect(),
        ),
    };
    let part_pins = part_pins.as_slice();
    let part_pin = |id: &str| part_pins.iter().any(|pin| pin.id == id);
    let mut nodes: Vec<Option<String>> = vec![None; ports.len()];
    if mapping.is_empty() && !options.strict {
        // KiCad without `Sim.Pins`: the pins in order.
        if part_pins.len() != ports.len() {
            return Err(fail(
                Pins,
                format!(
                    "the symbol has {} pins but {what} has {} ({}); add Sim.Pins",
                    part_pins.len(),
                    ports.len(),
                    ports.join(" ")
                ),
            ));
        }
        for (node, pin) in nodes.iter_mut().zip(part_pins) {
            *node = Some(pin.id.clone());
        }
    }
    let mut seen_pins = BTreeSet::new();
    for (pin, port) in &mapping {
        if !part_pin(pin) {
            return Err(fail(Pins, format!("the part has no pin {pin:?}")));
        }
        if !seen_pins.insert(pin) {
            return Err(fail(Pins, format!("pin {pin:?} is mapped twice")));
        }
        let index = ports
            .iter()
            .position(|p| p.eq_ignore_ascii_case(port))
            .ok_or_else(|| {
                fail(
                    Pins,
                    format!(
                        "{what} has no port {port:?}; its ports are {}",
                        ports.join(" ")
                    ),
                )
            })?;
        if let Some(other) = nodes[index].replace(pin.clone()) {
            return Err(fail(
                Pins,
                format!(
                    "port {} is mapped twice (pins {other:?} and {pin:?})",
                    ports[index]
                ),
            ));
        }
    }
    for pin in &binding.unused {
        if !part_pin(pin) {
            return Err(fail(Unused, format!("the part has no pin {pin:?}")));
        }
        if seen_pins.contains(pin) {
            return Err(fail(
                Unused,
                format!("pin {pin:?} is mapped to a port, so it cannot be unused"),
            ));
        }
    }
    let unmapped_ports: Vec<&str> = ports
        .iter()
        .zip(&nodes)
        .filter(|(_, node)| node.is_none())
        .map(|(port, _)| port.as_str())
        .collect();
    if !unmapped_ports.is_empty() {
        let mut message = format!(
            "no pin is mapped to the ports {} of {what}",
            unmapped_ports.join(", ")
        );
        let partial = ElementDef {
            device,
            suffix: binding.suffix.clone(),
            nodes: nodes
                .iter()
                .map(|n| n.clone().unwrap_or_default())
                .collect(),
            args: Args::Fixed(String::new()),
            models: Vec::new(),
            dialect: Dialect::Ngspice,
            info: Some(ModelInfo {
                path: path.clone(),
                name: found.name.clone(),
                model_type: None,
                ports: ports.clone(),
                params: Vec::new(),
                dialect: Dialect::Ngspice,
                dialect_reason: String::new(),
            }),
        };
        message.push_str(&pin_table(part_pins, std::slice::from_ref(&partial)));
        let free_pins: Vec<&PartPin> = part_pins
            .iter()
            .filter(|pin| !seen_pins.contains(&pin.id) && !binding.unused.contains(&pin.id))
            .collect();
        message.push_str(&suggestions(&free_pins, &unmapped_ports));
        return Err(fail(Pins, message));
    }
    let nodes: Vec<String> = nodes.into_iter().map(Option::unwrap).collect();
    let unmapped_pins: Vec<String> = if options.strict {
        Vec::new()
    } else {
        part_pins
            .iter()
            .filter(|pin| !nodes.contains(&pin.id) && !binding.unused.contains(&pin.id))
            .map(|pin| pin.id.clone())
            .collect()
    };

    // Parameters.
    if model_type.is_some() && !binding.params.is_empty() {
        return Err(fail(
            Params,
            format!("{what} is a .model; parameters are for subcircuits"),
        ));
    }
    let mut args = found.name.clone();
    let mut seen_params = BTreeSet::new();
    for (name, value) in &binding.params {
        if !defaults.iter().any(|(p, _)| p.eq_ignore_ascii_case(name)) {
            let known: Vec<String> = defaults.iter().map(|(p, v)| format!("{p}={v}")).collect();
            return Err(fail(
                Params,
                if known.is_empty() {
                    format!("{what} has no parameters")
                } else {
                    format!(
                        "{what} has no parameter {name:?}; its parameters are {}",
                        known.join(" ")
                    )
                },
            ));
        }
        if !seen_params.insert(name.to_ascii_lowercase()) {
            return Err(fail(Params, format!("parameter {name:?} is given twice")));
        }
        if value.is_empty()
            || value.contains(|c: char| c.is_whitespace() || c.is_control() || c == ';' || c == '$')
        {
            return Err(fail(
                Params,
                format!("the value {value:?} of {name} cannot be written in a deck"),
            ));
        }
        args.push_str(&format!(" {name}={value}"));
    }

    // Dialect.
    let hints: Vec<(&Path, &DialectHint)> = libs
        .iter()
        .flat_map(|(lib, _, file)| lib.hints.iter().map(move |hint| (file.as_path(), hint)))
        .collect();
    let describe = |hints: &[&(&Path, &DialectHint)]| -> String {
        let list: Vec<String> = hints
            .iter()
            .take(3)
            .map(|(file, hint)| {
                format!(
                    "{hint} of {}",
                    file.file_name().unwrap_or_default().to_string_lossy()
                )
            })
            .collect();
        list.join(", ")
    };
    let of = |dialect| {
        hints
            .iter()
            .filter(|(_, hint)| hint.dialect == dialect)
            .collect::<Vec<_>>()
    };
    let (pspice, ltspice) = (of(Dialect::Pspice), of(Dialect::Ltspice));
    let (dialect, dialect_reason) = match binding.dialect {
        DialectOption::Given(dialect) => (dialect, "given".to_string()),
        DialectOption::Auto => match (pspice.is_empty(), ltspice.is_empty()) {
            (true, true) => (
                Dialect::Ngspice,
                "no PSpice or LTspice constructs found".to_string(),
            ),
            (false, true) => (
                Dialect::Pspice,
                format!("detected from {}", describe(&pspice)),
            ),
            (true, false) => (
                Dialect::Ltspice,
                format!("detected from {}", describe(&ltspice)),
            ),
            (false, false) => {
                return Err(fail(
                    BindingField::Dialect,
                    format!(
                        "the library has both PSpice constructs ({}) and LTspice constructs ({}); \
                         give the dialect with `dialect: pspice` or `dialect: ltspice`",
                        describe(&pspice),
                        describe(&ltspice)
                    ),
                ));
            }
        },
    };
    if options.mode == ModelMode::Include {
        let includes = libs
            .iter()
            .flat_map(|(lib, section, _)| lib.includes(section.as_deref()));
        for include in includes {
            if include.pspice && dialect != Dialect::Pspice {
                return Err(fail(
                    BindingField::Dialect,
                    format!(
                        "the library includes {:?} with `.LIB`, which ngspice reads only in its PSpice mode; \
                         use `dialect: pspice` or `sim_model: inline`",
                        include.path
                    ),
                ));
            }
            if let Some(section) = &include.section
                && dialect == Dialect::Pspice
            {
                return Err(fail(
                    BindingField::Dialect,
                    format!(
                        "the library reads section {section} of {:?} with `.lib`, which ngspice's PSpice mode \
                         does not support; use `sim_model: inline`",
                        include.path
                    ),
                ));
            }
        }
    }

    let models = match options.mode {
        ModelMode::Include => vec![ModelDef::Include(path.clone())],
        ModelMode::Inline => {
            let mut definitions = dependencies;
            definitions.sort_by_key(|d| d.name.to_ascii_lowercase());
            definitions
                .into_iter()
                .map(|d| ModelDef::Inline {
                    name: d.name,
                    text: d.text,
                })
                .collect()
        }
    };
    let element = ElementDef {
        device,
        suffix: binding.suffix.clone(),
        nodes,
        args: Args::Fixed(args),
        models,
        dialect,
        info: Some(ModelInfo {
            path,
            name: found.name.clone(),
            model_type,
            ports,
            params: defaults,
            dialect,
            dialect_reason,
        }),
    };
    Ok((element, unmapped_pins))
}

/// The element letter and terminals of a device with a `.model` of `model_type` (lowercase).
pub fn model_device(model_type: &str) -> Option<(char, &'static [&'static str])> {
    Some(
        match model_type.split_whitespace().next().unwrap_or_default() {
            "d" => ('D', &["A", "K"]),
            "npn" | "pnp" => ('Q', &["C", "B", "E"]),
            "vdmos" => ('M', &["D", "G", "S"]),
            "nmos" | "pmos" => ('M', &["D", "G", "S", "B"]),
            "njf" | "pjf" => ('J', &["D", "G", "S"]),
            _ => return None,
        },
    )
}

/// A library file, the section that is read (`None`: outside the sections) and its path.
type LibFile = (Arc<SpiceLib>, Option<String>, PathBuf);

/// Reads a library and the files it includes, each with the section that is read.
fn read_library(path: &Path, resolver: &mut Resolver) -> Result<Vec<LibFile>, Error> {
    let mut libs = Vec::new();
    let mut queue = vec![(path.to_path_buf(), None::<String>)];
    let mut seen = BTreeSet::new();
    while let Some((file, section)) = queue.pop() {
        let key = (
            file.clone(),
            section.as_ref().map(|s| s.to_ascii_lowercase()),
        );
        if !seen.insert(key) || seen.len() > 32 {
            continue;
        }
        let lib = resolver.spice_lib(&file)?;
        if let Some(section) = &section
            && !lib.sections.iter().any(|s| s.eq_ignore_ascii_case(section))
        {
            return Err(Error::new(format!(
                "{} has no section {section:?}",
                file.display()
            )));
        }
        let dir = file.parent().map(PathBuf::from).unwrap_or_default();
        // Relative paths are relative to the including file, as in ngspice.
        let mut includes: Vec<_> = lib
            .includes(section.as_deref())
            .map(|include| (dir.join(&include.path), include.section.clone()))
            .collect();
        includes.reverse();
        queue.extend(includes);
        libs.push((lib, section, file));
    }
    Ok(libs)
}

/// The definition and the top-level definitions (also `.param` / `.func`) it refers to by name.
fn dependencies(definition: &Definition, libs: &[LibFile]) -> Vec<Definition> {
    let find_any = |name: &str| {
        libs.iter()
            .find_map(|(lib, section, _)| {
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
    out
}

/// Names that a definition may refer to: the words of its text after the keyword and its own name.
/// Names may contain `+` and `-` (`IN+`), which are also operators, so both forms are returned.
fn referenced_names(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('*'))
        .flat_map(|line| {
            line.split(|c: char| c.is_whitespace() || "(),={}'\"*/<>&|?:!^;".contains(c))
        })
        .filter(|word| !word.is_empty())
        .skip(2)
        .flat_map(|word| {
            std::iter::once(word).chain(word.split(['+', '-']).filter(|w| !w.is_empty()))
        })
}

/// The error for a name that is not in the library, with candidates.
fn not_found(name: &str, path: &Path, libs: &[LibFile]) -> Error {
    let definitions: Vec<&Definition> = libs
        .iter()
        .flat_map(|(lib, section, _)| lib.definitions(section.as_deref()))
        .filter(|d| {
            matches!(
                d.kind,
                DefinitionKind::Subckt { .. } | DefinitionKind::Model { .. }
            )
        })
        .collect();
    let mut message = format!("no .subckt or .model named {name:?} in {}", path.display());
    let wanted = normalize(name);
    let similar: Vec<&str> = definitions
        .iter()
        .map(|d| d.name.as_str())
        .filter(|candidate| {
            let candidate = normalize(candidate);
            candidate.contains(&wanted)
                || wanted.contains(&candidate)
                || edit_distance(&candidate, &wanted) <= 2
        })
        .take(8)
        .collect();
    if !similar.is_empty() {
        message.push_str(&format!("; similar names: {}", similar.join(", ")));
    }
    // Subcircuits no other definition uses are likely the products of the library.
    let used: BTreeSet<String> = definitions
        .iter()
        .flat_map(|d| {
            referenced_names(&d.text)
                .filter(|name| !name.eq_ignore_ascii_case(&d.name))
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        })
        .collect();
    let top: Vec<&str> = definitions
        .iter()
        .filter(|d| {
            matches!(d.kind, DefinitionKind::Subckt { .. })
                && !used.contains(&d.name.to_ascii_lowercase())
        })
        .map(|d| d.name.as_str())
        .take(10)
        .collect();
    if !top.is_empty() {
        message.push_str(&format!(
            "; subcircuits not used by others: {}",
            top.join(", ")
        ));
    }
    let encrypted: usize = libs.iter().map(|(lib, _, _)| lib.encrypted_blocks).sum();
    if encrypted > 0 {
        message.push_str(&format!(
            "; the library has {encrypted} encrypted blocks, which may hide it"
        ));
    }
    let sections: Vec<&str> = libs
        .iter()
        .flat_map(|(lib, _, _)| lib.sections.iter().map(String::as_str))
        .collect();
    if !sections.is_empty() {
        message.push_str(&format!(
            "; definitions in `.lib` sections ({}) are found only through `.lib file section`",
            sections.join(", ")
        ));
    }
    Error::new(message)
}

fn describe_pin(pin: &PartPin) -> String {
    match &pin.name {
        Some(name) if name != &pin.id => format!("{} ({name})", pin.id),
        _ => pin.id.clone(),
    }
}

/// A table of the pins and the ports they are mapped to, appended to an error message.
fn pin_table(pins: &[PartPin], elements: &[ElementDef]) -> String {
    let mut text = String::from("\npins and ports:");
    for pin in pins {
        let ports: Vec<String> = elements
            .iter()
            .filter_map(|element| {
                let info = element.info.as_ref()?;
                let index = element.nodes.iter().position(|node| node == &pin.id)?;
                let port = &info.ports[index];
                Some(if element.suffix.is_empty() {
                    port.clone()
                } else {
                    format!("{port} ({})", element.suffix)
                })
            })
            .collect();
        let ports = if ports.is_empty() {
            "-".to_string()
        } else {
            ports.join(", ")
        };
        text.push_str(&format!("\n  {:<16} => {ports}", describe_pin(pin)));
    }
    text
}

/// Mappings suggested from the names, to copy into `pins` after checking them.
fn suggestions(pins: &[&PartPin], ports: &[&str]) -> String {
    let mut by_name: BTreeMap<String, Vec<&PartPin>> = BTreeMap::new();
    for pin in pins {
        if let Some(name) = &pin.name {
            by_name.entry(normalize(name)).or_default().push(pin);
        }
    }
    let mut ports_by_name: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for port in ports {
        ports_by_name.entry(normalize(port)).or_default().push(port);
    }
    let mut lines = Vec::new();
    for pin in pins {
        let Some(key) = pin.name.as_deref().map(normalize) else {
            continue;
        };
        if let (Some([_]), Some([port])) = (
            by_name.get(&key).map(Vec::as_slice),
            ports_by_name.get(&key).map(Vec::as_slice),
        ) && !key.is_empty()
        {
            lines.push(format!("\n  {:?} => {:?},", pin.id, port));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "\nsuggested from the names (not applied; check them against the datasheet):{}",
        lines.concat()
    )
}

/// A name for matching pins and ports: uppercase, without `~{}/#` and a trailing `_N` / `_B` / `N` / `B`
/// (active-low markers); `+` and `-` are kept.
fn normalize(name: &str) -> String {
    let upper: String = name
        .chars()
        .filter(|c| !"~{}/#".contains(*c))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    for suffix in ["_N", "_B", "N", "B"] {
        if let Some(stem) = upper.strip_suffix(suffix)
            && stem.len() >= 3
        {
            return stem.to_string();
        }
    }
    upper
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let current = row[j + 1];
            row[j + 1] = (previous + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(current + 1);
            previous = current;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver() -> Resolver {
        Resolver::new(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"),
            |_| None,
        )
    }

    /// The pins of a TPS3899-like symbol: neither the order nor all names match VSUP01's ports
    /// (`CTR CTS GND RESET_B SENSE VDD`).
    fn symbol_pins() -> Vec<PartPin> {
        ["CTR", "CTS", "GND", "VDD", "SENSE", "RESET_N"]
            .iter()
            .enumerate()
            .map(|(i, name)| PartPin {
                id: (i + 1).to_string(),
                name: Some(name.to_string()),
            })
            .collect()
    }

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    fn strings(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    const VSUP_PINS: &[(&str, &str)] = &[
        ("1", "CTR"),
        ("2", "CTS"),
        ("3", "GND"),
        ("4", "VDD"),
        ("5", "SENSE"),
        ("6", "RESET_B"),
    ];

    fn vsup(pins: &[(&str, &str)]) -> ModelBinding {
        ModelBinding {
            library: "models/vsup01.lib".into(),
            name: "vsup01".into(),
            pins: pairs(pins),
            ..Default::default()
        }
    }

    fn run(
        bindings: &[ModelBinding],
        pins: &[PartPin],
        mode: ModelMode,
    ) -> Result<Bound, BindingError> {
        let options = BindOptions {
            pins: PartPins::Given(pins),
            strict: true,
            internal_connections: &[],
            mode,
        };
        bind(bindings, &options, &mut resolver())
    }

    fn error(bindings: &[ModelBinding]) -> (usize, BindingField, String) {
        let e = run(bindings, &symbol_pins(), ModelMode::Include).unwrap_err();
        (e.element, e.field, e.error.message)
    }

    #[test]
    fn test_subckt() {
        let mut binding = vsup(VSUP_PINS);
        binding.params = pairs(&[("vth", "0.6")]);
        let (elements, unused) =
            run(&[binding.clone()], &symbol_pins(), ModelMode::Include).unwrap();
        let element = &elements[0];
        assert_eq!(element.device, 'X');
        // In the order of the ports, not of the pin numbers.
        assert_eq!(element.nodes, strings(&["1", "2", "3", "6", "5", "4"]));
        assert_eq!(element.args, Args::Fixed("VSUP01 vth=0.6".into()));
        assert_eq!(element.dialect, Dialect::Pspice);
        let info = element.info.as_ref().unwrap();
        assert_eq!(
            info.dialect_reason,
            "detected from `.LIB file` (line 7) of vsup01.lib, `IF(` (line 15) of vsup01.lib, `&` (line 15) of vsup01.lib"
        );
        assert_eq!(info.params, pairs(&[("VTH", "0.5"), ("VDDMIN", "0.8")]));
        assert_eq!(
            element.models,
            vec![ModelDef::Include(
                resolver().manifest_dir.join("models/vsup01.lib")
            )]
        );
        assert!(unused.is_empty());

        // Inline: the definitions it uses, also from the file included with `.LIB`.
        let (elements, _) = run(&[binding], &symbol_pins(), ModelMode::Inline).unwrap();
        let names: Vec<&str> = elements[0]
            .models
            .iter()
            .map(|m| match m {
                ModelDef::Inline { name, .. } => name.as_str(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            names,
            [
                "D_VSUP_ESD",
                "VSUP01",
                "VSUP_DRIVE",
                "VSUP_OUTSTAGE",
                "VSUP_RON"
            ]
        );
    }

    #[test]
    fn test_model_and_lenient_pins() {
        let binding = ModelBinding {
            library: "models/clamp.lib".into(),
            name: "CLAMP".into(),
            pins: pairs(&[("2", "a"), ("1", "K")]),
            ..Default::default()
        };
        let pins: Vec<PartPin> = ["1", "2", "3"]
            .iter()
            .map(|id| PartPin {
                id: id.to_string(),
                name: None,
            })
            .collect();
        let options = BindOptions {
            pins: PartPins::Given(&pins),
            strict: false,
            internal_connections: &[],
            mode: ModelMode::Include,
        };
        let (elements, unused) =
            bind(std::slice::from_ref(&binding), &options, &mut resolver()).unwrap();
        assert_eq!(
            (elements[0].device, &elements[0].nodes),
            ('D', &strings(&["2", "1"]))
        );
        assert_eq!(
            elements[0].info.as_ref().unwrap().model_type.as_deref(),
            Some("d")
        );
        assert_eq!(elements[0].args, Args::Fixed("clamp".into()));
        // KiCad's `Sim.Pins`: the pins that are not mapped are unused.
        assert_eq!(unused, ["3"]);
        // Strict: they must be declared.
        let e = run(std::slice::from_ref(&binding), &pins, ModelMode::Include).unwrap_err();
        assert!(e.error.message.starts_with("pins not mapped to the model: 3; map them in `pins`, or declare them with `unused: [\"3\"]`"), "{}", e.error);
        let mut declared = binding.clone();
        declared.unused = strings(&["3"]);
        assert_eq!(
            run(&[declared], &pins, ModelMode::Include).unwrap().1,
            ["3"]
        );
        // ... or be connected to a mapped pin inside the part.
        let options = BindOptions {
            pins: PartPins::Given(&pins),
            strict: true,
            internal_connections: &[strings(&["1", "3"])],
            mode: ModelMode::Include,
        };
        assert!(
            bind(std::slice::from_ref(&binding), &options, &mut resolver())
                .unwrap()
                .1
                .is_empty()
        );
        // Positional without any mapping.
        let mut positional = binding;
        positional.pins.clear();
        let options = BindOptions {
            pins: PartPins::Given(&pins[..2]),
            strict: false,
            internal_connections: &[],
            mode: ModelMode::Include,
        };
        assert_eq!(
            bind(&[positional], &options, &mut resolver()).unwrap().0[0].nodes,
            ["1", "2"]
        );
    }

    #[test]
    fn test_unmapped_ports_with_suggestions() {
        let (_, field, message) = error(&[vsup(&VSUP_PINS[..5])]);
        assert_eq!(field, BindingField::Pins);
        assert_eq!(
            message,
            "no pin is mapped to the ports RESET_B of subcircuit VSUP01
pins and ports:
  1 (CTR)          => CTR
  2 (CTS)          => CTS
  3 (GND)          => GND
  4 (VDD)          => VDD
  5 (SENSE)        => SENSE
  6 (RESET_N)      => -
suggested from the names (not applied; check them against the datasheet):
  \"6\" => \"RESET_B\","
        );
        // Without any mapping, every port is suggested.
        let (_, _, message) = error(&[vsup(&[])]);
        assert!(
            message.contains("\"4\" => \"VDD\",\n  \"5\" => \"SENSE\",\n  \"6\" => \"RESET_B\","),
            "{message}"
        );
        assert!(
            normalize("~{RESET}") == "RESET"
                && normalize("VIN") == "VIN"
                && normalize("IN+") == "IN+"
        );
    }

    #[test]
    fn test_errors() {
        use BindingField::*;
        let mut pins = VSUP_PINS.to_vec();
        pins[5] = ("6", "RESET_N");
        let (_, field, message) = error(&[vsup(&pins)]);
        assert_eq!(field, Pins);
        assert_eq!(
            message,
            "subcircuit VSUP01 has no port \"RESET_N\"; its ports are CTR CTS GND RESET_B SENSE VDD"
        );
        pins[5] = ("6", "vdd");
        assert_eq!(
            error(&[vsup(&pins)]).2,
            "port VDD is mapped twice (pins \"4\" and \"6\")"
        );
        pins[5] = ("7", "RESET_B");
        assert_eq!(error(&[vsup(&pins)]).2, "the part has no pin \"7\"");
        pins[5] = ("5", "RESET_B");
        assert_eq!(error(&[vsup(&pins)]).2, "pin \"5\" is mapped twice");

        let mut binding = vsup(VSUP_PINS);
        binding.unused = strings(&["6"]);
        assert_eq!(
            error(&[binding]),
            (
                0,
                Unused,
                "pin \"6\" is mapped to a port, so it cannot be unused".into()
            )
        );

        let mut binding = vsup(VSUP_PINS);
        binding.params = pairs(&[("TD", "1m")]);
        assert_eq!(
            error(&[binding.clone()]).2,
            "subcircuit VSUP01 has no parameter \"TD\"; its parameters are VTH=0.5 VDDMIN=0.8"
        );
        binding.params = pairs(&[("VTH", "1 2")]);
        assert_eq!(
            error(&[binding]).2,
            "the value \"1 2\" of VTH cannot be written in a deck"
        );

        let mut binding = vsup(VSUP_PINS);
        binding.name = "VSUP1".into();
        let (_, field, message) = error(&[binding]);
        assert_eq!(field, Name);
        assert!(
            message.ends_with(
                "vsup01.lib; similar names: VSUP01; subcircuits not used by others: VSUP01"
            ),
            "{message}"
        );
        let mut binding = vsup(VSUP_PINS);
        binding.library = "models/none.lib".into();
        assert_eq!(error(&[binding]).1, Library);
    }

    #[test]
    fn test_unsupported_models() {
        use BindingField::*;
        let binding = |name: &str| ModelBinding {
            library: "models/vsup_unsupported.lib".into(),
            name: name.into(),
            pins: pairs(&[("1", "IN"), ("2", "OUT")]),
            ..Default::default()
        };
        let pins = &symbol_pins()[..2];
        let error = |binding: ModelBinding| {
            let e = run(&[binding], pins, ModelMode::Include).unwrap_err();
            (e.field, e.error.message)
        };
        assert_eq!(
            error(binding("vsup_digital")),
            (Name, "subcircuit VSUP_DIGITAL has `OPTIONAL:` ports (DPWR, DGND), which ngspice does not support".into())
        );
        assert_eq!(
            error(binding("vsup_locked")),
            (
                Name,
                "subcircuit VSUP_LOCKED is encrypted, which ngspice cannot read; ask the vendor for an unencrypted model"
                    .into()
            )
        );
        assert_eq!(
            error(binding("vsup_mixed")),
            (
                Dialect,
                "the library has both PSpice constructs (`IF(` (line 12) of vsup_unsupported.lib) and LTspice \
                 constructs (`uplim(` (line 13) of vsup_unsupported.lib); give the dialect with `dialect: pspice` \
                 or `dialect: ltspice`"
                    .into()
            )
        );
        let mut given = binding("vsup_mixed");
        given.dialect = DialectOption::Given(crate::spice_lib::Dialect::Ltspice);
        assert_eq!(
            run(&[given], pins, ModelMode::Include).unwrap().0[0].dialect,
            crate::spice_lib::Dialect::Ltspice
        );

        // `.LIB file` is read only in the PSpice mode.
        let mut ngspice = vsup(VSUP_PINS);
        ngspice.dialect = DialectOption::Given(crate::spice_lib::Dialect::Ngspice);
        let e = run(&[ngspice.clone()], &symbol_pins(), ModelMode::Include).unwrap_err();
        assert_eq!(e.field, Dialect);
        assert!(
            e.error
                .message
                .starts_with("the library includes \"vsup01_aux.lib\" with `.LIB`"),
            "{}",
            e.error
        );
        assert!(run(&[ngspice], &symbol_pins(), ModelMode::Inline).is_ok());
    }

    #[test]
    fn test_several_elements() {
        // Two copies of the same model sharing GND and VDD, like the units of a dual part.
        let pins: Vec<PartPin> = (1..=10)
            .map(|i| PartPin {
                id: i.to_string(),
                name: None,
            })
            .collect();
        let unit = |suffix: &str, first: usize| {
            let ids: Vec<String> = (first..first + 4).map(|i| i.to_string()).collect();
            ModelBinding {
                suffix: suffix.into(),
                ..vsup(&[
                    (&ids[0], "CTR"),
                    (&ids[1], "CTS"),
                    ("9", "GND"),
                    ("10", "VDD"),
                    (&ids[2], "SENSE"),
                    (&ids[3], "RESET_B"),
                ])
            }
        };
        let (elements, unused) =
            run(&[unit("A", 1), unit("B", 5)], &pins, ModelMode::Include).unwrap();
        assert!(unused.is_empty());
        assert_eq!(elements[0].nodes, strings(&["1", "2", "9", "4", "3", "10"]));
        assert_eq!(
            (elements[1].suffix.as_str(), elements[1].nodes[3].as_str()),
            ("B", "8")
        );
        let e = run(&[unit("A", 1), unit("a", 5)], &pins, ModelMode::Include).unwrap_err();
        assert_eq!(
            (e.element, e.field, e.error.message.as_str()),
            (1, BindingField::Suffix, "suffix \"a\" is used twice")
        );
        let e = run(&[unit("", 1), unit("", 5)], &pins, ModelMode::Include).unwrap_err();
        assert_eq!(
            e.error.message,
            "give each model a distinct `suffix` (e.g. \"A\", \"B\")"
        );
        // A pin left out of both is reported with the ports of every element.
        let e = run(&[unit("A", 1)], &pins, ModelMode::Include).unwrap_err();
        assert!(
            e.error
                .message
                .starts_with("pins not mapped to the model: 5, 6, 7, 8;"),
            "{}",
            e.error
        );
        assert!(
            e.error
                .message
                .contains("\n  9                => GND (A)\n"),
            "{}",
            e.error
        );
    }
}
