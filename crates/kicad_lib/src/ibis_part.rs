//! Checks the pins an `ibis_part!` asks for against an IBIS file at compile time: the component,
//! each pin and its model (or the model chosen from its `[Model Selector]`), the differential
//! pairs and series models, and that each model can be simulated at every corner.

use std::path::PathBuf;

use cuprite_wire_ibis::{
    Component, Corner, IbisFile, LookupError, Model, ModelLookupError, PinModel,
    spice::{Parasitics, series_subcircuit, subcircuit},
};

use crate::{Error, binding::similar_names, resolve::Resolver};

/// What `ibis_part!` asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IbisPartOptions {
    /// The IBIS file, as for `spice_part!` (relative to the crate, absolute, or with `${VAR}`).
    pub library: String,
    /// The `[Component]`; `None` for the only one of the file.
    pub component: Option<String>,
    /// The functions to generate, in order.
    pub pins: Vec<IbisPinOption>,
}

/// A function of the generated type: what it returns, of which pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IbisPinOption {
    /// The name of the function.
    pub name: String,
    pub kind: IbisPinKind,
    /// The pin (for a pair, either pin; for a series model, the first pin).
    pub pin: String,
    /// The model chosen from the `[Model Selector]` of the pin (of both pins of a pair).
    pub model: Option<String>,
}

/// What a function returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IbisPinKind {
    /// The buffer of a pin (`IbisBuffer`).
    Single,
    /// A differential pair of `[Diff Pin]` (`IbisDiffBuffer`).
    Diff,
    /// The series model between `pin` and this pin (`IbisSeries`).
    Series(String),
}

/// The option an error points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IbisKey {
    Library,
    Component,
    /// The `index`th entry of the pins.
    Pin {
        index: usize,
        field: IbisPinField,
    },
}

/// A part of an entry of the pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IbisPinField {
    /// The pin (the first pin of a series model).
    Pin,
    /// The second pin of a series model.
    Pin2,
    Model,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IbisPartError {
    pub key: IbisKey,
    pub error: Error,
}

/// The checked part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IbisPartDef {
    /// The absolute path of the file, which is read again at run time.
    pub path: PathBuf,
    pub component: String,
    pub manufacturer: Option<String>,
    /// `[IBIS Ver]` and `[File Rev]`.
    pub ibis_ver: String,
    pub file_rev: Option<String>,
    pub pins: Vec<IbisPinDef>,
}

/// A checked function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IbisPinDef {
    pub option: IbisPinOption,
    /// The pins and their buffers: one for a pin, the non-inverting then the inverting pin of
    /// a pair; empty for a series model.
    pub buffers: Vec<BufferDef>,
    /// The series model: of a series entry, or between the pins of a pair.
    pub series: Option<String>,
}

/// A pin and the model it uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferDef {
    pub pin: String,
    pub signal_name: String,
    /// The model used.
    pub model: String,
    /// `Model_type`.
    pub model_type: String,
    /// The `[Model Selector]` the pin names, if any.
    pub selector: Option<String>,
}

const CORNERS: [Corner; 3] = [Corner::Typ, Corner::Min, Corner::Max];

/// Reads the file and checks the pins asked for.
pub fn load_ibis_part(
    options: &IbisPartOptions,
    resolver: &mut Resolver,
) -> Result<IbisPartDef, IbisPartError> {
    let at = |key| move |error: Error| IbisPartError { key, error };
    let path = resolver
        .sim_library_path(&options.library)
        .map_err(at(IbisKey::Library))?;
    if !path.is_file() {
        resolver.track(&path);
        return Err(at(IbisKey::Library)(Error::new(format!(
            "IBIS file not found ({})",
            path.display()
        ))));
    }
    let file = resolver.ibis_file(&path).map_err(at(IbisKey::Library))?;
    let component =
        find_component(&file, options.component.as_deref()).map_err(at(IbisKey::Component))?;
    let pins = options
        .pins
        .iter()
        .enumerate()
        .map(|(index, option)| check_pin(&file, component, option, index))
        .collect::<Result<_, _>>()?;
    Ok(IbisPartDef {
        path,
        component: component.name.clone(),
        manufacturer: component.manufacturer.clone(),
        ibis_ver: file.header.ibis_ver.clone(),
        file_rev: file.header.file_rev.clone(),
        pins,
    })
}

fn component_names(file: &IbisFile) -> String {
    let names: Vec<&str> = file.components.iter().map(|c| c.name.as_str()).collect();
    names.join(", ")
}

fn find_component<'a>(file: &'a IbisFile, name: Option<&str>) -> Result<&'a Component, Error> {
    match name {
        Some(name) => file.find_component(name).map_err(|e| {
            let mut message = format!("{e}; the components of the file: {}", component_names(file));
            let similar = similar_names(name, file.components.iter().map(|c| c.name.as_str()));
            if !similar.is_empty() {
                message.push_str(&format!("; similar names: {}", similar.join(", ")));
            }
            Error::new(message)
        }),
        None => match file.components.as_slice() {
            [only] => Ok(only),
            [] => Err(Error::new("the file has no [Component]")),
            _ => Err(Error::new(format!(
                "the file has several components; choose one with `component`: {}",
                component_names(file)
            ))),
        },
    }
}

fn check_pin(
    file: &IbisFile,
    component: &Component,
    option: &IbisPinOption,
    index: usize,
) -> Result<IbisPinDef, IbisPartError> {
    let error = |field, error: Error| IbisPartError {
        key: IbisKey::Pin { index, field },
        error,
    };
    let lookup = |e: LookupError| {
        let field = match &e {
            LookupError::Model {
                error: ModelLookupError::NotSelectable { .. },
                ..
            } => IbisPinField::Model,
            _ => IbisPinField::Pin,
        };
        error(field, explain(file, component, &e))
    };
    let choice = option.model.as_deref();
    let mut buffers = Vec::new();
    let mut series = None;
    match &option.kind {
        IbisPinKind::Single => {
            buffers.push(check_buffer(file, component, &option.pin, choice).map_err(
                |e| match e {
                    Checked::Lookup(e) => lookup(e),
                    Checked::Simulation(e) => error(IbisPinField::Pin, e),
                },
            )?);
        }
        IbisPinKind::Diff => {
            let diff = component.find_diff_pin(&option.pin).map_err(lookup)?;
            for pin in [&diff.pin, &diff.inv_pin] {
                buffers.push(
                    check_buffer(file, component, pin, choice).map_err(|e| match e {
                        Checked::Lookup(e) => lookup(e),
                        Checked::Simulation(e) => error(IbisPinField::Pin, e),
                    })?,
                );
            }
            series = check_series(file, component, &diff.pin, &diff.inv_pin)
                .map_err(|e| error(IbisPinField::Pin, e))?;
        }
        IbisPinKind::Series(pin2) => {
            if option.model.is_some() {
                return Err(error(
                    IbisPinField::Model,
                    Error::new("a series model has no [Model Selector] to choose from"),
                ));
            }
            for (pin, field) in [(&option.pin, IbisPinField::Pin), (pin2, IbisPinField::Pin2)] {
                if component.pin(pin).is_none() {
                    let e = LookupError::NoPin {
                        component: component.name.clone(),
                        pin: pin.clone(),
                    };
                    return Err(error(field, explain(file, component, &e)));
                }
            }
            series = check_series(file, component, &option.pin, pin2)
                .map_err(|e| error(IbisPinField::Pin, e))?;
            if series.is_none() {
                let e = LookupError::NoSeriesPinMapping {
                    component: component.name.clone(),
                    pin1: option.pin.clone(),
                    pin2: pin2.clone(),
                };
                return Err(error(IbisPinField::Pin, explain(file, component, &e)));
            }
        }
    }
    Ok(IbisPinDef {
        option: option.clone(),
        buffers,
        series,
    })
}

enum Checked {
    Lookup(LookupError),
    Simulation(Error),
}

/// The buffer of a pin, simulated at every corner.
fn check_buffer(
    file: &IbisFile,
    component: &Component,
    pin: &str,
    choice: Option<&str>,
) -> Result<BufferDef, Checked> {
    let (p, model) = file
        .buffer_model(component, pin, choice)
        .map_err(Checked::Lookup)?;
    for corner in CORNERS {
        subcircuit("", model, &Parasitics::of_pin(component, p, corner), corner)
            .map_err(|e| Checked::Simulation(cannot_simulate(pin, model, corner, e)))?;
    }
    let selector = match &p.model {
        PinModel::Named(name) if file.model_selector(name).is_some() => Some(name.clone()),
        _ => None,
    };
    Ok(BufferDef {
        pin: pin.to_string(),
        signal_name: p.signal_name.clone(),
        model: model.name.clone(),
        model_type: model.model_type.name().to_string(),
        selector,
    })
}

/// The name of the series model between the pins, simulated at every corner, if any.
fn check_series(
    file: &IbisFile,
    component: &Component,
    pin1: &str,
    pin2: &str,
) -> Result<Option<String>, Error> {
    let model = file
        .series_model(component, pin1, pin2)
        .map_err(|e| Error::new(e.to_string()))?;
    let Some(model) = model else {
        return Ok(None);
    };
    for corner in CORNERS {
        series_subcircuit("", model, corner)
            .map_err(|e| cannot_simulate(pin1, model, corner, e))?;
    }
    Ok(Some(model.name.clone()))
}

fn cannot_simulate(pin: &str, model: &Model, corner: Corner, error: impl ToString) -> Error {
    Error::new(format!(
        "pin `{pin}`: model `{}` cannot be simulated at the {} corner: {}",
        model.name,
        match corner {
            Corner::Typ => "typ",
            Corner::Min => "min",
            Corner::Max => "max",
        },
        error.to_string()
    ))
}

/// The message of a lookup error, with what the file has instead.
fn explain(file: &IbisFile, component: &Component, error: &LookupError) -> Error {
    let mut message = error.to_string();
    match error {
        LookupError::NoPin { pin, .. } => {
            let by_signal: Vec<String> = component
                .pins
                .iter()
                .filter(|p| p.signal_name.eq_ignore_ascii_case(pin))
                .map(|p| format!("`{}`", p.name))
                .collect();
            if !by_signal.is_empty() {
                message.push_str(&format!(
                    "; pins are named by number: the signal `{pin}` is on the pin {}",
                    by_signal.join(", ")
                ));
            } else if pin.chars().count() >= 3 {
                // Short pin numbers are all alike.
                let similar = similar_names(pin, component.pins.iter().map(|p| p.name.as_str()));
                if !similar.is_empty() {
                    message.push_str(&format!("; similar pins: {}", similar.join(", ")));
                }
            }
        }
        LookupError::Model {
            error: ModelLookupError::NotSelectable { name, .. },
            ..
        } => match file.model_selector(name) {
            Some(selector) => {
                message.push_str(&format!("; the models of [Model Selector] `{name}`:"));
                for option in &selector.options {
                    message.push_str(&format!("\n  {}", option.model));
                    if !option.description.is_empty() {
                        message.push_str(&format!("  ({})", option.description));
                    }
                }
            }
            None => message.push_str(&format!(
                "; the pin uses the model `{name}`, which is not a [Model Selector]"
            )),
        },
        LookupError::NoDiffPin { .. } => {
            let pairs: Vec<String> = component
                .diff_pins
                .iter()
                .map(|d| format!("{}/{}", d.pin, d.inv_pin))
                .collect();
            message.push_str(&if pairs.is_empty() {
                "; the component has no [Diff Pin]".to_string()
            } else {
                format!("; the pairs: {}", pairs.join(", "))
            });
        }
        LookupError::NoSeriesPinMapping { .. } => {
            let mappings: Vec<String> = component
                .series_pin_mappings
                .iter()
                .map(|m| format!("{}-{} ({})", m.pin1, m.pin2, m.model))
                .collect();
            message.push_str(&if mappings.is_empty() {
                "; the component has no [Series Pin Mapping]".to_string()
            } else {
                format!("; the mappings: {}", mappings.join(", "))
            });
        }
        _ => {}
    }
    Error::new(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::fixture_resolver;

    fn pin(name: &str, kind: IbisPinKind, pin: &str, model: Option<&str>) -> IbisPinOption {
        IbisPinOption {
            name: name.into(),
            kind,
            pin: pin.into(),
            model: model.map(Into::into),
        }
    }

    fn options(component: Option<&str>, pins: Vec<IbisPinOption>) -> IbisPartOptions {
        IbisPartOptions {
            library: "models/ibis_part.ibs".into(),
            component: component.map(Into::into),
            pins,
        }
    }

    fn load(options: &IbisPartOptions) -> Result<IbisPartDef, IbisPartError> {
        load_ibis_part(options, &mut fixture_resolver())
    }

    /// The key and message of the error.
    fn error(options: &IbisPartOptions) -> (IbisKey, String) {
        let e = load(options).unwrap_err();
        (e.key, e.error.message)
    }

    fn pin_key(index: usize, field: IbisPinField) -> IbisKey {
        IbisKey::Pin { index, field }
    }

    #[test]
    fn test_load() {
        let mut resolver = fixture_resolver();
        let part = load_ibis_part(
            &options(
                Some("Chip"),
                vec![
                    pin("tx", IbisPinKind::Single, "3", None),
                    pin("io", IbisPinKind::Single, "4", Some("in_cmos")),
                    pin("io_default", IbisPinKind::Single, "4", None),
                    pin("rx", IbisPinKind::Diff, "2", None),
                    pin("term", IbisPinKind::Series("8".into()), "7", None),
                ],
            ),
            &mut resolver,
        )
        .unwrap();
        assert!(part.path.ends_with("tests/fixtures/models/ibis_part.ibs"));
        assert!(part.path.is_absolute());
        assert_eq!(resolver.files, std::slice::from_ref(&part.path));
        assert_eq!(part.component, "Chip");
        assert_eq!(part.manufacturer.as_deref(), Some("Example Corp."));
        assert_eq!(part.file_rev.as_deref(), Some("1.2"));
        let models = |i: usize| -> Vec<(&str, &str)> {
            part.pins[i]
                .buffers
                .iter()
                .map(|b| (b.pin.as_str(), b.model.as_str()))
                .collect()
        };
        assert_eq!(models(0), [("3", "out_3s")]);
        assert_eq!(part.pins[0].buffers[0].model_type, "3-state");
        assert_eq!(part.pins[0].buffers[0].signal_name, "TX");
        assert_eq!(models(1), [("4", "in_cmos")]);
        assert_eq!(part.pins[1].buffers[0].selector.as_deref(), Some("io_sel"));
        // The first model of the selector is the default.
        assert_eq!(models(2), [("4", "out_3s")]);
        // A pair given by its inverting pin: the non-inverting pin first, and the termination.
        assert_eq!(models(3), [("1", "in_cmos"), ("2", "in_cmos")]);
        assert_eq!(part.pins[3].series.as_deref(), Some("term"));
        assert!(models(4).is_empty());
        assert_eq!(part.pins[4].series.as_deref(), Some("term"));
    }

    #[test]
    fn test_component() {
        let (key, message) = error(&options(None, vec![]));
        assert_eq!(key, IbisKey::Component);
        assert_eq!(
            message,
            "the file has several components; choose one with `component`: Chip, Other"
        );
        let (key, message) = error(&options(Some("chip"), vec![]));
        assert_eq!(key, IbisKey::Component);
        assert_eq!(
            message,
            "no [Component] `chip`; the components of the file: Chip, Other; similar names: Chip"
        );
    }

    #[test]
    fn test_library() {
        let mut opts = options(None, vec![]);
        opts.library = "models/none.ibs".into();
        let (key, message) = error(&opts);
        assert_eq!(key, IbisKey::Library);
        assert!(message.starts_with("IBIS file not found ("), "{message}");
        opts.library = "models/amp.lib".into();
        let (key, message) = error(&opts);
        assert_eq!(key, IbisKey::Library);
        assert!(message.contains("amp.lib: "), "{message}");
    }

    #[test]
    fn test_pin_errors() {
        let one =
            |kind, p: &str, model| error(&options(Some("Chip"), vec![pin("f", kind, p, model)]));
        assert_eq!(
            one(IbisPinKind::Single, "TX", None),
            (
                pin_key(0, IbisPinField::Pin),
                "component `Chip` has no pin `TX`; pins are named by number: the signal `TX` is on the pin `3`"
                    .into()
            )
        );
        assert_eq!(
            one(IbisPinKind::Single, "5", None),
            (
                pin_key(0, IbisPinField::Pin),
                "pin `5` has no model: it is a POWER pin".into()
            )
        );
        assert_eq!(
            one(IbisPinKind::Single, "4", Some("fast")),
            (
                pin_key(0, IbisPinField::Model),
                "pin `4` has no model: `fast` is not a model of `io_sel`; the models of [Model Selector] `io_sel`:\
                 \n  out_3s  (3-state output)\n  in_cmos  (CMOS input)"
                    .into()
            )
        );
        assert_eq!(
            one(IbisPinKind::Single, "3", Some("in_cmos")).1,
            "pin `3` has no model: `in_cmos` is not a model of `out_3s`; the pin uses the model `out_3s`, which is not a [Model Selector]"
        );
        let (key, message) = one(IbisPinKind::Single, "6", None);
        assert_eq!(key, pin_key(0, IbisPinField::Pin));
        assert!(
            message.starts_with("pin `6`: model `broken` cannot be simulated at the typ corner: "),
            "{message}"
        );
        assert_eq!(
            one(IbisPinKind::Diff, "3", None).1,
            "pin `3` of component `Chip` is not in [Diff Pin]; the pairs: 1/2"
        );
        assert_eq!(
            one(IbisPinKind::Series("1".into()), "7", None),
            (
                pin_key(0, IbisPinField::Pin),
                "component `Chip` has no [Series Pin Mapping] between pins `7` and `1`; the mappings: 2-1 (term), 7-8 (term)"
                    .into()
            )
        );
        assert_eq!(
            one(IbisPinKind::Series("9".into()), "7", None).0,
            pin_key(0, IbisPinField::Pin2)
        );
        assert_eq!(
            one(IbisPinKind::Series("8".into()), "7", Some("term")).0,
            pin_key(0, IbisPinField::Model)
        );
    }
}
