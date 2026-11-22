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
    spice_lib::{Definition, DefinitionKind, Dialect, DialectHint, SpiceLib, strip_comment},
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
    /// device for a `.model` (`A` `K`, `C` `B` `E`, `D` `G` `S` \[`B`\]).
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
    /// KiCad's `Sim.Device` for the model: `SUBCKT`, or that of the [`ModelDevice`].
    pub sim_device: &'static str,
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
        crate::naming::check_element_suffix(suffix).map_err(error)?;
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
            message.push_str(&pin_table(pins, |id| {
                elements
                    .iter()
                    .filter_map(|element| {
                        let info = element.info.as_ref()?;
                        let index = element.nodes.iter().position(|node| node == id)?;
                        let port = &info.ports[index];
                        Some(if element.suffix.is_empty() {
                            port.clone()
                        } else {
                            format!("{port} ({})", element.suffix)
                        })
                    })
                    .collect()
            }));
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

fn fail(field: BindingField, message: String) -> FieldError {
    (field, Error::new(message))
}

fn bind_one(
    binding: &ModelBinding,
    options: &BindOptions,
    resolver: &mut Resolver,
) -> Result<(ElementDef, Vec<String>), FieldError> {
    use BindingField::Library;
    let path = resolver
        .sim_library_path(&binding.library)
        .map_err(|e| (Library, e))?;
    let libs = read_library(&path, resolver).map_err(|e| (Library, e))?;
    let found = find_definition(&binding.name, &path, &libs)?;
    let model = model_of(&found)?;
    let dependencies = dependencies(&found, &libs);
    check_encrypted(&found, &dependencies)?;

    let (part_pins, mapping) = match options.pins {
        PartPins::Given(pins) => (pins.to_vec(), binding.pins.clone()),
        PartPins::Ports => (
            model
                .ports
                .iter()
                .map(|port| PartPin {
                    id: port.clone(),
                    name: None,
                })
                .collect(),
            model
                .ports
                .iter()
                .map(|port| (port.clone(), port.clone()))
                .collect(),
        ),
    };
    let (nodes, unmapped_pins) = map_ports(
        &part_pins,
        &mapping,
        &binding.unused,
        &model.ports,
        options.strict,
        &model.what,
    )?;
    let args = instance_args(&found.name, &model, &binding.params)?;
    let (dialect, dialect_reason) = choose_dialect(binding.dialect, &libs)?;
    let models = match options.mode {
        ModelMode::Include => {
            check_includes(&libs, dialect)?;
            vec![ModelDef::Include(path.clone())]
        }
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
        device: model.device,
        suffix: binding.suffix.clone(),
        nodes,
        args: Args::Fixed(args),
        models,
        dialect,
        info: Some(ModelInfo {
            path,
            name: found.name.clone(),
            model_type: model.model_type,
            sim_device: model.sim_device,
            ports: model.ports,
            params: model.defaults,
            dialect,
            dialect_reason,
        }),
    };
    Ok((element, unmapped_pins))
}

/// The `.subckt` or `.model` named `name` in the library or the files it includes.
fn find_definition(name: &str, path: &Path, libs: &[LibFile]) -> Result<Definition, FieldError> {
    libs.iter()
        .find_map(|(lib, section, _)| lib.find_in(name, section.as_deref()))
        .cloned()
        .ok_or_else(|| (BindingField::Name, not_found(name, path, libs)))
}

/// What a definition is as a device.
struct Model {
    /// `subcircuit NAME` / `model NAME`, for messages.
    what: String,
    device: char,
    /// The ports of the `.subckt`, or the terminals of the device.
    ports: Vec<String>,
    model_type: Option<String>,
    sim_device: &'static str,
    /// The parameters of the `.subckt` with their defaults.
    defaults: Vec<(String, String)>,
}

fn model_of(found: &Definition) -> Result<Model, FieldError> {
    match &found.kind {
        DefinitionKind::Subckt {
            ports,
            optional_ports,
            params,
        } => {
            let what = format!("subcircuit {}", found.name);
            if !optional_ports.is_empty() {
                let names: Vec<&str> = optional_ports
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect();
                return Err(fail(
                    BindingField::Name,
                    format!(
                        "{what} has `OPTIONAL:` ports ({}), which ngspice does not support",
                        names.join(", ")
                    ),
                ));
            }
            Ok(Model {
                what,
                device: 'X',
                ports: ports.clone(),
                model_type: None,
                sim_device: "SUBCKT",
                defaults: params.clone(),
            })
        }
        DefinitionKind::Model { model_type } => {
            let what = format!("model {}", found.name);
            let device = model_device(model_type, &found.text).ok_or_else(|| {
                fail(
                    BindingField::Name,
                    format!(
                        "{what} has the type {model_type:?}; supported types are D, NPN, PNP, NMOS, PMOS, VDMOS, NJF and PJF"
                    ),
                )
            })?;
            Ok(Model {
                what,
                device: device.letter,
                ports: device.terminals.iter().map(|t| t.to_string()).collect(),
                model_type: Some(model_type.clone()),
                sim_device: device.sim_device,
                defaults: Vec::new(),
            })
        }
        _ => unreachable!("find_in returns subcircuits and models"),
    }
}

/// An error if the definition or one it uses is encrypted.
fn check_encrypted(found: &Definition, dependencies: &[Definition]) -> Result<(), FieldError> {
    let Some(locked) = dependencies.iter().find(|d| d.encrypted) else {
        return Ok(());
    };
    let detail = if locked.name.eq_ignore_ascii_case(&found.name) {
        String::new()
    } else {
        format!(" (used by {})", found.name)
    };
    Err(fail(
        BindingField::Name,
        format!(
            "{} {}{detail} is encrypted, which ngspice cannot read; ask the vendor for an unencrypted model",
            if matches!(locked.kind, DefinitionKind::Model { .. }) {
                "model"
            } else {
                "subcircuit"
            },
            locked.name
        ),
    ))
}

/// Maps the pins of a part to the ports of a model (`what`, for messages): `mapping` is pin =>
/// port, ports are matched case-insensitively, and every port needs a pin. Not `strict`, an empty
/// mapping connects the pins in order (KiCad without `Sim.Pins`).
///
/// Returns the pins in port order and, not `strict`, the pins neither mapped nor in `unused`.
pub(crate) fn map_ports(
    part_pins: &[PartPin],
    mapping: &[(String, String)],
    unused: &[String],
    ports: &[String],
    strict: bool,
    what: &str,
) -> Result<(Vec<String>, Vec<String>), FieldError> {
    use BindingField::{Pins, Unused};
    let part_pin = |id: &str| part_pins.iter().any(|pin| pin.id == id);
    let mut nodes: Vec<Option<String>> = vec![None; ports.len()];
    if mapping.is_empty() && !strict {
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
    for (pin, port) in mapping {
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
    if let Some(pin) = unused.iter().find(|pin| !part_pin(pin)) {
        return Err(fail(Unused, format!("the part has no pin {pin:?}")));
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
        message.push_str(&pin_table(part_pins, |pin| {
            nodes
                .iter()
                .position(|node| node.as_deref() == Some(pin))
                .map(|index| ports[index].clone())
                .into_iter()
                .collect()
        }));
        let free_pins: Vec<&PartPin> = part_pins
            .iter()
            .filter(|pin| !seen_pins.contains(&pin.id) && !unused.contains(&pin.id))
            .collect();
        message.push_str(&suggestions(&free_pins, &unmapped_ports));
        return Err(fail(Pins, message));
    }
    let nodes: Vec<String> = nodes.into_iter().map(Option::unwrap).collect();
    let unmapped_pins = if strict {
        Vec::new()
    } else {
        part_pins
            .iter()
            .filter(|pin| !nodes.contains(&pin.id) && !unused.contains(&pin.id))
            .map(|pin| pin.id.clone())
            .collect()
    };
    Ok((nodes, unmapped_pins))
}

/// The arguments of the element: the name of the model and the parameters of the `.subckt`.
fn instance_args(
    name: &str,
    model: &Model,
    params: &[(String, String)],
) -> Result<String, FieldError> {
    let what = &model.what;
    let fail = |message| fail(BindingField::Params, message);
    if model.model_type.is_some() && !params.is_empty() {
        return Err(fail(format!(
            "{what} is a .model; parameters are for subcircuits"
        )));
    }
    let mut args = name.to_string();
    let mut seen = BTreeSet::new();
    for (name, value) in params {
        let defaults = &model.defaults;
        if !defaults.iter().any(|(p, _)| p.eq_ignore_ascii_case(name)) {
            let known: Vec<String> = defaults.iter().map(|(p, v)| format!("{p}={v}")).collect();
            return Err(fail(if known.is_empty() {
                format!("{what} has no parameters")
            } else {
                format!(
                    "{what} has no parameter {name:?}; its parameters are {}",
                    known.join(" ")
                )
            }));
        }
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(fail(format!("parameter {name:?} is given twice")));
        }
        check_param_value(name, value).map_err(|e| (BindingField::Params, e))?;
        args.push_str(&format!(" {name}={value}"));
    }
    Ok(args)
}

/// Checks that a parameter value can be written in a deck as one token. Expressions (`{...}`,
/// `'...'`) are left to ngspice.
pub(crate) fn check_param_value(name: &str, value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.contains(|c: char| {
            c.is_whitespace() || c.is_control() || matches!(c, ';' | '$' | '\\' | '"')
        })
    {
        return Err(Error::new(format!(
            "the value {value:?} of {name} cannot be written in a deck"
        )));
    }
    Ok(())
}

/// The dialect of the model, and why it was chosen.
fn choose_dialect(
    option: DialectOption,
    libs: &[LibFile],
) -> Result<(Dialect, String), FieldError> {
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
    Ok(match option {
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
    })
}

/// An error if ngspice cannot read the includes of an included library in `dialect`.
fn check_includes(libs: &[LibFile], dialect: Dialect) -> Result<(), FieldError> {
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
    Ok(())
}

/// A device with a `.model`: its KiCad `Sim.Device`, element letter and terminals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelDevice {
    pub sim_device: &'static str,
    pub letter: char,
    pub terminals: &'static [&'static str],
}

const DIODE: &[&str] = &["A", "K"];
const BJT: &[&str] = &["C", "B", "E"];
const MOSFET: &[&str] = &["D", "G", "S", "B"];
const THREE_TERMINAL_FET: &[&str] = &["D", "G", "S"];

/// `.model` types (lowercase), with their devices.
const MODEL_DEVICES: &[(&str, ModelDevice)] = &[
    ("d", device("D", 'D', DIODE)),
    ("npn", device("NPN", 'Q', BJT)),
    ("pnp", device("PNP", 'Q', BJT)),
    ("nmos", device("NMOS", 'M', MOSFET)),
    ("pmos", device("PMOS", 'M', MOSFET)),
    // N-channel unless the parameters have `pchan`.
    ("vdmos", device("NMOS", 'M', THREE_TERMINAL_FET)),
    ("njf", device("NJFET", 'J', THREE_TERMINAL_FET)),
    ("pjf", device("PJFET", 'J', THREE_TERMINAL_FET)),
];

const fn device(
    sim_device: &'static str,
    letter: char,
    terminals: &'static [&'static str],
) -> ModelDevice {
    ModelDevice {
        sim_device,
        letter,
        terminals,
    }
}

/// The device of a `.model` of `model_type` (lowercase) defined by `text` (the `.model`
/// statement, which tells the channel of a VDMOS).
pub fn model_device(model_type: &str, text: &str) -> Option<ModelDevice> {
    let (_, mut device) = *MODEL_DEVICES.iter().find(|(t, _)| *t == model_type)?;
    if model_type == "vdmos" && has_pchan(text) {
        device.sim_device = "PMOS";
    }
    Some(device)
}

/// Whether the parameters of a `.model` have the `pchan` flag.
fn has_pchan(text: &str) -> bool {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('*'))
        .map(strip_comment)
        .flat_map(|line| line.split(|c: char| c.is_whitespace() || "(),=".contains(c)))
        .filter(|word| !word.is_empty())
        .skip(3)
        .any(|word| word.eq_ignore_ascii_case("pchan"))
}

/// A library file, the section that is read (`None`: outside the sections) and its path.
type LibFile = (Arc<SpiceLib>, Option<String>, PathBuf);

/// The most files a library may read with its includes, to stop include loops through paths that
/// keep growing (`.include sub/self.lib` with `sub` a link to `.`).
const MAX_LIBRARY_FILES: usize = 256;

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
        if !seen.insert(key) {
            continue;
        }
        if seen.len() > MAX_LIBRARY_FILES {
            return Err(Error::new(format!(
                "{} includes more than {MAX_LIBRARY_FILES} files; do the includes loop?",
                path.display()
            )));
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
        .map(strip_comment)
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

/// A table of the pins and the ports they are mapped to (`ports_of` a pin ID), appended to an
/// error message.
fn pin_table(pins: &[PartPin], ports_of: impl Fn(&str) -> Vec<String>) -> String {
    let mut text = String::from("\npins and ports:");
    for pin in pins {
        let ports = ports_of(&pin.id);
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
    use crate::test_util::{fixture_resolver, pairs, strings};

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
        bind(bindings, &options, &mut fixture_resolver())
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
                fixture_resolver().manifest_dir.join("models/vsup01.lib")
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
        let (elements, unused) = bind(
            std::slice::from_ref(&binding),
            &options,
            &mut fixture_resolver(),
        )
        .unwrap();
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
            bind(
                std::slice::from_ref(&binding),
                &options,
                &mut fixture_resolver()
            )
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
            bind(&[positional], &options, &mut fixture_resolver())
                .unwrap()
                .0[0]
                .nodes,
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
    fn test_include_loop() {
        // A chain of includes longer than the limit is an error, not a silently cut search.
        let dir = std::env::temp_dir().join(format!("cuprite_include_loop_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let count = MAX_LIBRARY_FILES + 1;
        for i in 0..count {
            let text = if i + 1 < count {
                format!(".include l{}.lib\n", i + 1)
            } else {
                ".model clamp D\n".to_string()
            };
            std::fs::write(dir.join(format!("l{i}.lib")), text).unwrap();
        }
        let binding = ModelBinding {
            library: dir.join("l0.lib").to_string_lossy().into_owned(),
            name: "clamp".into(),
            pins: pairs(&[("1", "A"), ("2", "K")]),
            ..Default::default()
        };
        let pins = &symbol_pins()[..2];
        let e = run(&[binding], pins, ModelMode::Include).unwrap_err();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(e.field, BindingField::Library);
        assert!(
            e.error
                .message
                .ends_with("includes more than 256 files; do the includes loop?"),
            "{}",
            e.error
        );
    }

    #[test]
    fn test_referenced_names_skip_comments() {
        let names: Vec<&str> =
            referenced_names(".subckt s a b\nx1 a b inner ; other\n* third\n+ $ fourth\n.ends")
                .collect();
        assert!(names.contains(&"inner"), "{names:?}");
        for comment in ["other", "third", "fourth"] {
            assert!(!names.contains(&comment), "{names:?}");
        }
    }

    #[test]
    fn test_model_devices() {
        let device = |text: &str| {
            let model_type = text.split([' ', '(']).nth(2).unwrap().to_ascii_lowercase();
            model_device(&model_type, text).map(|d| (d.sim_device, d.letter, d.terminals.len()))
        };
        assert_eq!(device(".model a D"), Some(("D", 'D', 2)));
        assert_eq!(device(".model a NMOS level=1"), Some(("NMOS", 'M', 4)));
        assert_eq!(device(".model a VDMOS(Kp=1)"), Some(("NMOS", 'M', 3)));
        assert_eq!(
            device(".model a VDMOS(Kp=1\n+ PCHAN)"),
            Some(("PMOS", 'M', 3))
        );
        assert_eq!(device(".model pchan VDMOS NCHAN"), Some(("NMOS", 'M', 3)));
        assert_eq!(device(".model a VDMOS PCHAN"), Some(("PMOS", 'M', 3)));
        // Inline comments are not parameters.
        assert_eq!(
            device(".model a VDMOS(Kp=1) $ pchan"),
            Some(("NMOS", 'M', 3))
        );
        assert_eq!(
            device(".model a VDMOS(Kp=1) // pchan"),
            Some(("NMOS", 'M', 3))
        );
        assert_eq!(device(".model a SW"), None);
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
