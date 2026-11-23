//! Finding the component, pin and model a simulation asks for, with errors that say what is
//! missing.

use std::fmt;

use crate::{Component, DiffPin, IbisFile, Model, ModelLookupError, Pin, PinModel};

/// The reason a lookup of [`IbisFile::find_component`], [`IbisFile::buffer_model`],
/// [`Component::find_diff_pin`] or [`IbisFile::series_model`] found nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LookupError {
    /// The file has no `[Component]` of the name.
    NoComponent(String),
    /// The component has no pin of the name.
    NoPin {
        /// The component.
        component: String,
        /// The pin.
        pin: String,
    },
    /// The pin is a supply, ground or unconnected pin.
    NotABuffer {
        /// The pin.
        pin: String,
        /// What the pin connects to: [`PinModel::Power`], [`PinModel::Gnd`] or
        /// [`PinModel::Nc`].
        model: PinModel,
    },
    /// The pin names a model the file does not have, or the model asked for is not one of its
    /// `[Model Selector]`.
    Model {
        /// The pin.
        pin: String,
        /// What is wrong with the model.
        error: ModelLookupError,
    },
    /// The pin is in no row of `[Diff Pin]`.
    NoDiffPin {
        /// The component.
        component: String,
        /// The pin.
        pin: String,
    },
    /// No row of `[Series Pin Mapping]` joins the pins.
    NoSeriesPinMapping {
        /// The component.
        component: String,
        /// The first pin.
        pin1: String,
        /// The second pin.
        pin2: String,
    },
}

impl fmt::Display for LookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LookupError::NoComponent(name) => write!(f, "no [Component] `{name}`"),
            LookupError::NoPin { component, pin } => {
                write!(f, "component `{component}` has no pin `{pin}`")
            }
            LookupError::NotABuffer { pin, model } => {
                let kind = match model {
                    PinModel::Power => "POWER",
                    PinModel::Gnd => "GND",
                    _ => "NC",
                };
                write!(f, "pin `{pin}` has no model: it is a {kind} pin")
            }
            LookupError::Model { pin, error } => write!(f, "pin `{pin}` has no model: {error}"),
            LookupError::NoDiffPin { component, pin } => {
                write!(
                    f,
                    "pin `{pin}` of component `{component}` is not in [Diff Pin]"
                )
            }
            LookupError::NoSeriesPinMapping {
                component,
                pin1,
                pin2,
            } => write!(
                f,
                "component `{component}` has no [Series Pin Mapping] between pins `{pin1}` and `{pin2}`"
            ),
        }
    }
}

impl std::error::Error for LookupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LookupError::Model { error, .. } => Some(error),
            _ => None,
        }
    }
}

impl IbisFile {
    /// The component with the name.
    ///
    /// # Errors
    /// [`LookupError::NoComponent`] if the file has none.
    pub fn find_component(&self, name: &str) -> Result<&Component, LookupError> {
        self.component(name)
            .ok_or_else(|| LookupError::NoComponent(name.to_string()))
    }

    /// The pin `pin` of `component` and the model it uses ([`IbisFile::pin_model`]): for a pin
    /// naming a `[Model Selector]`, `choice` or else the default model of the selector.
    ///
    /// # Errors
    /// If the component has no such pin, the pin is a supply, ground or `NC` pin, or the model
    /// is missing or not one of the selector.
    pub fn buffer_model<'a>(
        &'a self,
        component: &'a Component,
        pin: &str,
        choice: Option<&str>,
    ) -> Result<(&'a Pin, &'a Model), LookupError> {
        let p = component.pin(pin).ok_or_else(|| LookupError::NoPin {
            component: component.name.clone(),
            pin: pin.to_string(),
        })?;
        let model = match &p.model {
            PinModel::Named(name) => {
                self.pin_model(name, choice)
                    .map_err(|error| LookupError::Model {
                        pin: pin.to_string(),
                        error,
                    })?
            }
            other => {
                return Err(LookupError::NotABuffer {
                    pin: pin.to_string(),
                    model: other.clone(),
                });
            }
        };
        Ok((p, model))
    }

    /// The model that `[Series Pin Mapping]` of `component` puts between the pins `pin1` and
    /// `pin2` (in either order), `None` if no row joins them.
    ///
    /// # Errors
    /// If the row names a model the file does not have.
    pub fn series_model<'a>(
        &'a self,
        component: &Component,
        pin1: &str,
        pin2: &str,
    ) -> Result<Option<&'a Model>, LookupError> {
        let Some(mapping) = component.series_pin_mapping(pin1, pin2) else {
            return Ok(None);
        };
        self.pin_model(&mapping.model, None)
            .map(Some)
            .map_err(|error| LookupError::Model {
                pin: mapping.pin1.clone(),
                error,
            })
    }
}

impl Component {
    /// The differential pair of the pin ([`Component::diff_pin`]).
    ///
    /// # Errors
    /// [`LookupError::NoDiffPin`] if the pin is in no row of `[Diff Pin]`.
    pub fn find_diff_pin(&self, pin: &str) -> Result<&DiffPin, LookupError> {
        self.diff_pin(pin).ok_or_else(|| LookupError::NoDiffPin {
            component: self.name.clone(),
            pin: pin.to_string(),
        })
    }
}
