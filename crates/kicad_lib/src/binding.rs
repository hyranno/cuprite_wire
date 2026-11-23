//! Binding a model of a SPICE library (a `.subckt` or a `.model`) to the pins of a part.
//!
//! The three ways a part gets a library model share this: a symbol's `Sim.*` fields, the `sim`
//! block of `kicad_part!`, and `spice_part!`. Pins are mapped to ports by name; the order of the
//! nodes is always the order of the ports in the `.subckt` line (or of the device's terminals).
//! The bodies of the models are not interpreted, except for the files their XSPICE code models
//! read, which are resolved relative to the library.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

mod devices;
mod diagnostics;
mod library;

pub use devices::{ModelDevice, model_device};
pub(crate) use diagnostics::similar_names;
use diagnostics::{describe_pin, not_found, pin_table, suggestions};
use library::{LibFile, dependencies, read_library};
#[cfg(test)]
use {diagnostics::normalize, library::MAX_LIBRARY_FILES};

use crate::{
    Error,
    resolve::Resolver,
    sim::{Args, ElementDef, Elements, ModelDef, ModelMode},
    spice_lib::{Definition, DefinitionKind, Dialect, DialectHint, FileRef, file_refs},
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

pub fn bind(
    bindings: &[ModelBinding],
    options: &BindOptions,
    resolver: &mut Resolver,
) -> Result<Elements, BindingError> {
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
    Ok(Elements {
        elements,
        unused_pins: unused,
    })
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
    let (found, file) = find_definition(&binding.name, &path, &libs)?;
    let model = model_of(&found)?;
    let dependencies = dependencies(&found, &file, &libs);
    check_encrypted(&found, &dependencies)?;
    let files = model_files(&found, &dependencies, resolver)?;

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
            check_relative_files(&found, &files)?;
            vec![ModelDef::Include(path.clone())]
        }
        ModelMode::Inline => {
            let mut definitions: Vec<Definition> = dependencies
                .into_iter()
                .zip(files)
                .map(|((d, _), files)| Definition {
                    text: with_absolute_files(&d.text, &files),
                    ..d
                })
                .collect();
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

/// The `.subckt` or `.model` named `name` in the library or the files it includes, with the file
/// it is in.
fn find_definition(
    name: &str,
    path: &Path,
    libs: &[LibFile],
) -> Result<(Definition, PathBuf), FieldError> {
    libs.iter()
        .find_map(|(lib, section, file)| {
            lib.find_in(name, section.as_deref())
                .map(|d| (d.clone(), file.clone()))
        })
        .ok_or_else(|| (BindingField::Name, not_found(name, path, libs)))
}

/// A file named by a code model, resolved.
struct ModelFile {
    reference: FileRef,
    /// The definition it is in.
    definition: String,
    /// Absolute: relative paths are relative to the library file the definition is in.
    path: PathBuf,
}

/// The files named by the code models of the definitions (`d_state`'s `state_file`, ...), for
/// each definition. They must exist, and are tracked for rebuilds.
fn model_files(
    found: &Definition,
    dependencies: &[(Definition, PathBuf)],
    resolver: &mut Resolver,
) -> Result<Vec<Vec<ModelFile>>, FieldError> {
    dependencies
        .iter()
        .map(|(definition, library)| {
            file_refs(&definition.text)
                .into_iter()
                .map(|reference| {
                    let dir = library.parent().unwrap_or(Path::new(""));
                    let path = resolver.track(&dir.join(&reference.path));
                    if !path.is_file() {
                        return Err(fail(
                            BindingField::Library,
                            format!(
                                "model {} ({}{}) reads {:?} ({}), which does not exist",
                                reference.model,
                                reference.model_type,
                                used_by(definition, found),
                                reference.path,
                                path.display()
                            ),
                        ));
                    }
                    Ok(ModelFile {
                        reference,
                        definition: definition.name.clone(),
                        path,
                    })
                })
                .collect()
        })
        .collect()
}

/// `, used by NAME` when `definition` is not the model itself.
fn used_by(definition: &Definition, found: &Definition) -> String {
    if definition.name.eq_ignore_ascii_case(&found.name) {
        String::new()
    } else {
        format!(", used by {}", found.name)
    }
}

/// An error for a file named by a relative path in an included library: ngspice would open it
/// relative to the directory it runs in.
fn check_relative_files(found: &Definition, files: &[Vec<ModelFile>]) -> Result<(), FieldError> {
    let Some(file) = files
        .iter()
        .flatten()
        .find(|file| Path::new(&file.reference.path).is_relative())
    else {
        return Ok(());
    };
    let reference = &file.reference;
    let in_definition = if file.definition.eq_ignore_ascii_case(&reference.model) {
        String::new()
    } else {
        format!(" in {}", file.definition)
    };
    Err(fail(
        BindingField::Library,
        format!(
            "model {}{in_definition} ({}, used by {}) names its {} by the relative path {:?}, which \
             ngspice resolves against the directory it runs in; use `sim_model: inline` (the path is \
             made absolute: {}), or write an absolute path in the library",
            reference.model,
            reference.model_type,
            found.name,
            reference.param,
            reference.path,
            file.path.display()
        ),
    ))
}

/// `text` with the files of its code models written as absolute paths.
fn with_absolute_files(text: &str, files: &[ModelFile]) -> String {
    let mut text = text.to_string();
    for file in files.iter().rev() {
        let path = file.path.to_string_lossy();
        text.replace_range(file.reference.range.clone(), &format!("\"{path}\""));
    }
    text
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
fn check_encrypted(
    found: &Definition,
    dependencies: &[(Definition, PathBuf)],
) -> Result<(), FieldError> {
    let Some((locked, _)) = dependencies.iter().find(|(d, _)| d.encrypted) else {
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
    ) -> Result<Elements, BindingError> {
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
        let Elements {
            elements,
            unused_pins: unused,
        } = run(&[binding.clone()], &symbol_pins(), ModelMode::Include).unwrap();
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
        let Elements { elements, .. } = run(&[binding], &symbol_pins(), ModelMode::Inline).unwrap();
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
        let Elements {
            elements,
            unused_pins: unused,
        } = bind(
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
            run(&[declared], &pins, ModelMode::Include)
                .unwrap()
                .unused_pins,
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
            .unused_pins
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
                .elements[0]
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
            run(&[given], pins, ModelMode::Include).unwrap().elements[0].dialect,
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
    fn test_code_model_files() {
        let counter = |name: &str| ModelBinding {
            library: "models/xspice/counter2.lib".into(),
            name: name.into(),
            pins: pairs(&[("1", "clk"), ("2", "q0"), ("3", "q1")]),
            ..Default::default()
        };
        let pins = &symbol_pins()[..3];
        // With the separators of the platform, as the resolved paths (the manifest directory
        // ends with `/tests/fixtures`, also on Windows).
        let dir: PathBuf = fixture_resolver()
            .manifest_dir
            .join("models")
            .join("xspice")
            .components()
            .collect();
        let state_file = dir.join("counter2.in");

        // Inlined, the relative paths are made absolute, relative to the file of the definition.
        let mut resolver = fixture_resolver();
        let options = BindOptions {
            pins: PartPins::Given(pins),
            strict: true,
            internal_connections: &[],
            mode: ModelMode::Inline,
        };
        // `..` is kept, as for includes (`states` may be a link).
        let ext_state_file = dir.join("states").join("..").join("counter2.in");
        for (name, path) in [("counter2", &state_file), ("counter2_ext", &ext_state_file)] {
            let elements = bind(&[counter(name)], &options, &mut resolver).unwrap();
            let texts: Vec<&str> = elements.elements[0]
                .models
                .iter()
                .map(|m| match m {
                    ModelDef::Inline { text, .. } => text.as_str(),
                    other => panic!("{other:?}"),
                })
                .collect();
            let expected = format!("state_file = \"{}\")", path.display());
            assert_eq!(
                texts.iter().filter(|t| t.contains(&expected)).count(),
                1,
                "{texts:?}"
            );
        }
        // The state files are tracked for rebuilds.
        for path in [&state_file, &ext_state_file] {
            assert!(resolver.files.contains(path), "{:?}", resolver.files);
        }

        // Included, ngspice would look for them in the directory it runs in.
        let e = run(&[counter("counter2_ext")], pins, ModelMode::Include).unwrap_err();
        assert_eq!(e.field, BindingField::Library);
        assert_eq!(
            e.error.message,
            format!(
                "model count2_ext (d_state, used by counter2_ext) names its state_file by the relative \
                 path \"../counter2.in\", which ngspice resolves against the directory it runs in; use \
                 `sim_model: inline` (the path is made absolute: {}), or write an absolute path in the \
                 library",
                ext_state_file.display()
            )
        );
        let e = run(&[counter("counter2")], pins, ModelMode::Include).unwrap_err();
        assert!(
            e.error
                .message
                .starts_with("model count2 in counter2 (d_state, used by counter2) names"),
            "{}",
            e.error
        );

        let mut missing = counter("missing");
        missing.pins = pairs(&[("1", "clk"), ("2", "q")]);
        let e = run(&[missing], &pins[..2], ModelMode::Inline).unwrap_err();
        assert_eq!(e.field, BindingField::Library);
        assert_eq!(
            e.error.message,
            format!(
                "model nothing (d_state) reads \"missing.in\" ({}), which does not exist",
                dir.join("missing.in").display()
            )
        );
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
        let Elements {
            elements,
            unused_pins: unused,
        } = run(&[unit("A", 1), unit("B", 5)], &pins, ModelMode::Include).unwrap();
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
