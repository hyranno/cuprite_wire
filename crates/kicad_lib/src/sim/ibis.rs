//! `Sim.Device IBIS`: a pin (or a differential pair) of an IBIS file, driven or received, as
//! KiCad 10 simulates it.
//!
//! The model is given by the `Sim.*` fields (`Sim.Library`, `Sim.Name` for the component,
//! `Sim.Ibis.Pin`, `Sim.Ibis.Model`, `Sim.Ibis.Diff`, `Sim.Type`) or by the `ibis` block of
//! `kicad_part!`, and checked with [`load_ibis_part`]. The stimulus of a driver (`Sim.Params`) is
//! not simulated: the generated part plays the sequence it is given, and the DC and rectangular
//! stimuli are only offered as a sequence ([`Stimulus`]).

use super::{SimFields, map_pins};
use crate::{
    Error,
    ibis_part::{
        IbisPartDef, IbisPartKey, IbisPartOptions, IbisPinField, IbisPinKind, IbisPinOption,
        load_ibis_part,
    },
    resolve::Resolver,
    symbol::Symbol,
    value,
};

/// What the IBIS model of a part is: from the `Sim.*` fields or the `ibis` block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IbisSource {
    /// The IBIS file, as `Sim.Library`.
    pub library: String,
    /// The `[Component]`; `None` for the only one of the file.
    pub component: Option<String>,
    /// The pin (of a pair, either pin).
    pub pin: String,
    /// The model chosen from the `[Model Selector]` of the pin.
    pub model: Option<String>,
    /// A differential pair of `[Diff Pin]`.
    pub diff: bool,
    /// Drives the pin (KiCad's driver types), rather than receiving.
    pub driver: bool,
}

impl IbisSource {
    /// The part of `cuprite_wire::spice::ibis` simulating the model.
    pub fn inner(&self) -> IbisInner {
        const SINGLE: [&str; 2] = ["GND", "IN/OUT"];
        const DIFF: [&str; 3] = ["GND", "+", "-"];
        let (name, buffer, kicad_pins, pins, description): (_, _, &[&str], &[&str], _) =
            match (self.diff, self.driver) {
                (false, true) => (
                    "IbisDriver",
                    "IbisBuffer",
                    &SINGLE,
                    &["GND", "OUT"],
                    "the driver",
                ),
                (false, false) => (
                    "IbisReceiver",
                    "IbisBuffer",
                    &SINGLE,
                    &["GND", "IN"],
                    "the receiver",
                ),
                (true, true) => (
                    "IbisDiffDriver",
                    "IbisDiffBuffer",
                    &DIFF,
                    &["GND", "P", "N"],
                    "the differential driver",
                ),
                (true, false) => (
                    "IbisDiffReceiver",
                    "IbisDiffBuffer",
                    &DIFF,
                    &["GND", "P", "N"],
                    "the differential receiver",
                ),
            };
        IbisInner {
            name,
            buffer,
            kicad_pins,
            pins,
            description,
        }
    }

    /// KiCad's model pins: `GND IN/OUT`, or `GND + -` for a differential pair.
    pub fn kicad_pins(&self) -> &'static [&'static str] {
        self.inner().kicad_pins
    }
}

/// The part of `cuprite_wire::spice::ibis` simulating a model, as the generated code names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IbisInner {
    /// The type (`IbisDriver`, ...).
    pub name: &'static str,
    /// What its `buffer()` returns.
    pub buffer: &'static str,
    /// KiCad's model pins.
    pub kicad_pins: &'static [&'static str],
    /// The pin constants of the type, in the order of `kicad_pins`.
    pub pins: &'static [&'static str],
    /// What it is, for the documentation.
    pub description: &'static str,
}

/// The IBIS model of a part, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct IbisDef {
    pub source: IbisSource,
    /// The file and the buffers, with a single entry in `pins`.
    pub part: IbisPartDef,
    /// Pin numbers (pin IDs once the part is made) in the order of KiCad's model pins
    /// ([`IbisSource::kicad_pins`]).
    pub nodes: Vec<String>,
    /// Pins not connected to the model.
    pub unused_pins: Vec<String>,
    /// The stimulus of `Sim.Params`, for a DC or rectangular driver whose stimulus is complete.
    pub stimulus: Option<Stimulus>,
    /// The `Sim.Params` choosing a corner per parameter (`vcc`, `rpin`, `lpin`, `cpin`), which
    /// are not simulated: one corner is chosen for the whole model.
    pub corner_params: Vec<(String, String)>,
}

/// The stimulus of a KiCad IBIS driver.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stimulus {
    /// `DCDRIVER`: a constant level (`dc`, high-impedance by default).
    Dc(DcLevel),
    /// `RECTDRIVER`: low, then `n` periods of high for `ton` and low for `toff`, starting at `td`.
    Rect {
        ton: f64,
        toff: f64,
        td: f64,
        n: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DcLevel {
    High,
    Low,
    HiZ,
}

/// The `Sim.Params` choosing the corner of a part of the model.
const CORNER_PARAMS: [&str; 4] = ["vcc", "rpin", "lpin", "cpin"];

/// The model given by the `Sim.*` fields of a symbol.
pub(super) fn from_fields(
    symbol: &Symbol,
    fields: &SimFields,
    resolver: &mut Resolver,
) -> Result<IbisDef, Error> {
    let sim_type = fields.sim_type.as_deref().unwrap_or_default();
    let (driver, stimulus_params): (bool, &[&str]) = match sim_type {
        "DEVICE" => (false, &[]),
        "DCDRIVER" => (true, &["dc"]),
        "RECTDRIVER" => (true, &["ton", "toff", "td", "n"]),
        "PRBSDRIVER" => (true, &["f0", "n"]),
        _ => {
            return Err(Error::new(format!(
                "Sim.Type {sim_type:?} of Sim.Device IBIS is not supported; KiCad 10 has DEVICE, \
                 DCDRIVER, RECTDRIVER and PRBSDRIVER (the IBIS symbols of KiCad's library have \
                 an older type and no file: give the model with the `ibis` block)"
            )));
        }
    };
    let Some(library) = fields.library else {
        return Err(Error::new(
            "Sim.Device IBIS needs Sim.Library (the IBIS file); or give the model with the `ibis` block",
        ));
    };
    let field = |name| symbol.property(name).filter(|v| !v.is_empty());
    let Some(pin) = field("Sim.Ibis.Pin") else {
        return Err(Error::new(
            "Sim.Device IBIS needs Sim.Ibis.Pin (the pin of the component)",
        ));
    };
    let diff = match field("Sim.Ibis.Diff") {
        None | Some("0") => false,
        Some("1") => true,
        Some(other) => {
            return Err(Error::new(format!(
                "Sim.Ibis.Diff {other:?} is neither 0 nor 1"
            )));
        }
    };

    let mut corner_params = Vec::new();
    let mut stimulus = Vec::new();
    for (name, value) in &fields.params {
        let key = name.to_ascii_lowercase();
        if CORNER_PARAMS.contains(&key.as_str()) {
            corner_params.push((name.clone(), value.clone()));
        } else if stimulus_params.contains(&key.as_str()) {
            stimulus.push((key, value.as_str()));
        } else {
            let mut known: Vec<&str> = stimulus_params.to_vec();
            known.extend(CORNER_PARAMS);
            return Err(Error::new(format!(
                "Sim.Params {name:?} is not a parameter of Sim.Type {sim_type} (it has {})",
                known.join(", ")
            )));
        }
    }
    let stimulus = match sim_type {
        "DCDRIVER" => Some(dc_stimulus(&stimulus)?),
        "RECTDRIVER" => rect_stimulus(&stimulus)?,
        _ => None,
    };

    let source = IbisSource {
        library: library.to_string(),
        component: fields.model_name.map(str::to_string),
        pin: pin.to_string(),
        model: field("Sim.Ibis.Model").map(str::to_string),
        diff,
        driver,
    };
    let mut def = ibis_def(symbol, source, resolver).map_err(|(_, e)| e)?;
    def.stimulus = stimulus;
    def.corner_params = corner_params;
    Ok(def)
}

/// The part of [`IbisSource`] an error is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IbisField {
    Library,
    Component,
    Pin,
    Model,
    Diff,
    Driver,
    /// The symbol (its `Sim.Pins`).
    Symbol,
}

/// The model of `source`, connected to the symbol by its `Sim.Pins`.
pub fn ibis_def(
    symbol: &Symbol,
    source: IbisSource,
    resolver: &mut Resolver,
) -> Result<IbisDef, (IbisField, Error)> {
    let options = IbisPartOptions {
        library: source.library.clone(),
        component: source.component.clone(),
        pins: vec![IbisPinOption {
            name: "buffer".to_string(),
            kind: if source.diff {
                IbisPinKind::Diff
            } else {
                IbisPinKind::Single
            },
            pin: source.pin.clone(),
            model: source.model.clone(),
        }],
    };
    let part = load_ibis_part(&options, resolver).map_err(|e| {
        let field = match e.key {
            IbisPartKey::Library => IbisField::Library,
            IbisPartKey::Component => IbisField::Component,
            IbisPartKey::Pin {
                field: IbisPinField::Model,
                ..
            } => IbisField::Model,
            IbisPartKey::Pin { .. } => IbisField::Pin,
        };
        (field, e.error)
    })?;
    if source.driver
        && let Some(buffer) = part.pins[0].buffers.iter().find(|b| !b.driver)
    {
        return Err((
            IbisField::Driver,
            Error::new(format!(
                "pin `{}` has the model `{}` ({}), which does not drive; receive it instead",
                buffer.pin, buffer.model, buffer.model_type
            )),
        ));
    }
    let (nodes, unused_pins) =
        map_pins(symbol, source.kicad_pins()).map_err(|e| (IbisField::Symbol, e))?;
    Ok(IbisDef {
        source,
        part,
        nodes,
        unused_pins,
        stimulus: None,
        corner_params: Vec::new(),
    })
}

fn dc_stimulus(params: &[(String, &str)]) -> Result<Stimulus, Error> {
    let level = match params.iter().find(|(k, _)| k == "dc").map(|(_, v)| *v) {
        None => DcLevel::HiZ,
        Some(v) if v.eq_ignore_ascii_case("high") => DcLevel::High,
        Some(v) if v.eq_ignore_ascii_case("low") => DcLevel::Low,
        Some(v) if v.eq_ignore_ascii_case("hi-z") => DcLevel::HiZ,
        Some(v) => {
            return Err(Error::new(format!(
                "Sim.Params dc={v} is not high, low or hi-Z"
            )));
        }
    };
    Ok(Stimulus::Dc(level))
}

/// The rectangular stimulus, if `ton` and `toff` are given (KiCad's defaults of 0 make no wave).
fn rect_stimulus(params: &[(String, &str)]) -> Result<Option<Stimulus>, Error> {
    let get = |key: &str| params.iter().find(|(k, _)| k == key).map(|(_, v)| *v);
    let time = |key: &str| -> Result<Option<f64>, Error> {
        get(key)
            .map(|v| {
                value::to_f64(v)
                    .filter(|t| *t >= 0.0)
                    .ok_or_else(|| Error::new(format!("Sim.Params {key}={v} is not a time")))
            })
            .transpose()
    };
    let n = match get("n") {
        None => 1,
        Some(v) => v
            .parse::<u32>()
            .map_err(|_| Error::new(format!("Sim.Params n={v} is not a number of periods")))?,
    };
    let (ton, toff, td) = (time("ton")?, time("toff")?, time("td")?);
    Ok(match (ton, toff) {
        (Some(ton), Some(toff)) if ton > 0.0 && toff > 0.0 => Some(Stimulus::Rect {
            ton,
            toff,
            td: td.unwrap_or(0.0),
            n,
        }),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sim::{
            SpiceDef, Value,
            tests::{convert, props, symbol},
        },
        test_util::{fixture_resolver, pairs, strings},
    };

    const LIBRARY: &str = "models/kicad_ibis.ibs";

    fn ibis(fields: &[(&str, &str)], pins: &[(&str, &str)]) -> Result<IbisDef, Error> {
        let mut all = vec![
            ("Sim.Device", "IBIS"),
            ("Sim.Library", LIBRARY),
            ("Sim.Name", "Board"),
        ];
        all.extend(fields);
        let symbol = symbol(&props(&all), pins);
        let value = Value::Fixed("IBIS".into());
        match convert(&symbol, "U", value, &mut fixture_resolver())? {
            SpiceDef::Ibis(def) => Ok(*def),
            other => panic!("{other:?}"),
        }
    }

    const TWO_PINS: &[(&str, &str)] = &[("1", "REF"), ("2", "~")];

    #[test]
    fn test_driver() {
        let def = ibis(
            &[
                ("Sim.Type", "RECTDRIVER"),
                ("Sim.Ibis.Pin", "1"),
                ("Sim.Pins", "1=GND 2=IN/OUT"),
                ("Sim.Params", "ton=10n toff=20n td=5n n=3 vcc=max RPIN=min"),
            ],
            TWO_PINS,
        )
        .unwrap();
        assert_eq!(
            def.source,
            IbisSource {
                library: LIBRARY.into(),
                component: Some("Board".into()),
                pin: "1".into(),
                model: None,
                diff: false,
                driver: true,
            }
        );
        assert_eq!(def.nodes, strings(&["1", "2"]));
        assert!(def.unused_pins.is_empty());
        let Some(Stimulus::Rect { ton, toff, td, n }) = def.stimulus else {
            panic!("{:?}", def.stimulus);
        };
        assert!((ton - 10e-9).abs() < 1e-18 && (toff - 20e-9).abs() < 1e-18);
        assert!((td - 5e-9).abs() < 1e-18 && n == 3);
        assert_eq!(def.corner_params, pairs(&[("vcc", "max"), ("RPIN", "min")]));
        let buffer = &def.part.pins[0].buffers[0];
        assert_eq!((buffer.model.as_str(), buffer.driver), ("out_3s", true));
    }

    #[test]
    fn test_receiver_and_pins() {
        // Pins in another order, a pin left out, and the model of a selector.
        let def = ibis(
            &[
                ("Sim.Type", "DEVICE"),
                ("Sim.Ibis.Pin", "5"),
                ("Sim.Ibis.Model", "in_cmos"),
                ("Sim.Pins", "2=GND 1=IN/OUT"),
            ],
            &[("1", "~"), ("2", "~"), ("3", "NC")],
        )
        .unwrap();
        assert!(!def.source.driver);
        assert_eq!(def.nodes, strings(&["2", "1"]));
        assert_eq!(def.unused_pins, strings(&["3"]));
        assert_eq!(def.stimulus, None);
        assert_eq!(def.part.pins[0].buffers[0].model, "in_cmos");
    }

    #[test]
    fn test_diff_pins_by_number() {
        // `Sim.Pins` of KiCad's library symbols name the model pins by number.
        let def = ibis(
            &[
                ("Sim.Type", "DCDRIVER"),
                ("Sim.Ibis.Pin", "4"),
                ("Sim.Ibis.Diff", "1"),
                ("Sim.Pins", "1=1 2=2 3=3"),
                ("Sim.Params", "dc=low"),
            ],
            &[("1", "REF"), ("2", "+"), ("3", "-")],
        )
        .unwrap();
        assert!(def.source.diff);
        assert_eq!(def.nodes, strings(&["1", "2", "3"]));
        assert_eq!(def.stimulus, Some(Stimulus::Dc(DcLevel::Low)));
        let pins: Vec<&str> = def.part.pins[0]
            .buffers
            .iter()
            .map(|b| b.pin.as_str())
            .collect();
        assert_eq!(pins, ["3", "4"]);
    }

    #[test]
    fn test_stimulus() {
        let stimulus = |sim_type, params| {
            ibis(
                &[
                    ("Sim.Type", sim_type),
                    ("Sim.Ibis.Pin", "1"),
                    ("Sim.Params", params),
                ],
                TWO_PINS,
            )
            .map(|def| def.stimulus)
        };
        assert_eq!(
            stimulus("DCDRIVER", "").unwrap(),
            Some(Stimulus::Dc(DcLevel::HiZ))
        );
        assert_eq!(
            stimulus("DCDRIVER", "dc=High").unwrap(),
            Some(Stimulus::Dc(DcLevel::High))
        );
        assert_eq!(
            stimulus("DCDRIVER", "dc=hi-Z").unwrap(),
            Some(Stimulus::Dc(DcLevel::HiZ))
        );
        // KiCad's defaults of 0 make no wave.
        assert_eq!(stimulus("RECTDRIVER", "ton=10n").unwrap(), None);
        assert_eq!(
            stimulus("RECTDRIVER", "ton=10n toff=10n").unwrap(),
            Some(Stimulus::Rect {
                ton: 10e-9,
                toff: 10e-9,
                td: 0.0,
                n: 1
            })
        );
        assert_eq!(stimulus("PRBSDRIVER", "f0=10M n=20").unwrap(), None);
        for (sim_type, params, message) in [
            (
                "DCDRIVER",
                "dc=on",
                "Sim.Params dc=on is not high, low or hi-Z",
            ),
            ("RECTDRIVER", "ton=x", "Sim.Params ton=x is not a time"),
            (
                "RECTDRIVER",
                "n=1.5",
                "Sim.Params n=1.5 is not a number of periods",
            ),
            (
                "PRBSDRIVER",
                "ton=1n",
                "Sim.Params \"ton\" is not a parameter of Sim.Type PRBSDRIVER (it has f0, n, vcc, rpin, lpin, cpin)",
            ),
            (
                "DEVICE",
                "dc=high",
                "Sim.Params \"dc\" is not a parameter of Sim.Type DEVICE (it has vcc, rpin, lpin, cpin)",
            ),
        ] {
            assert_eq!(stimulus(sim_type, params).unwrap_err().message, message);
        }
    }

    #[test]
    fn test_errors() {
        let error = |fields: &[(&str, &str)]| ibis(fields, TWO_PINS).unwrap_err().message;
        assert!(
            error(&[("Sim.Type", "IBISDRIVERDC"), ("Sim.Ibis.Pin", "1")])
                .starts_with("Sim.Type \"IBISDRIVERDC\" of Sim.Device IBIS is not supported")
        );
        assert_eq!(
            error(&[("Sim.Type", "DEVICE")]),
            "Sim.Device IBIS needs Sim.Ibis.Pin (the pin of the component)"
        );
        assert_eq!(
            error(&[
                ("Sim.Type", "DEVICE"),
                ("Sim.Ibis.Pin", "1"),
                ("Sim.Ibis.Diff", "yes")
            ]),
            "Sim.Ibis.Diff \"yes\" is neither 0 nor 1"
        );
        assert_eq!(
            error(&[("Sim.Type", "DCDRIVER"), ("Sim.Ibis.Pin", "2")]),
            "pin `2` has the model `in_cmos` (Input), which does not drive; receive it instead"
        );
        assert!(
            error(&[("Sim.Type", "DEVICE"), ("Sim.Ibis.Pin", "9")]).contains("pin `9`"),
            "{}",
            error(&[("Sim.Type", "DEVICE"), ("Sim.Ibis.Pin", "9")])
        );
        assert!(
            error(&[
                ("Sim.Type", "DEVICE"),
                ("Sim.Ibis.Pin", "1"),
                ("Sim.Pins", "1=GND 2=OUT")
            ])
            .contains("no port \"OUT\"")
        );
    }

    #[test]
    fn test_without_library() {
        let symbol = symbol(
            &props(&[
                ("Sim.Device", "IBIS"),
                ("Sim.Type", "DEVICE"),
                ("Sim.Ibis.Pin", "1"),
            ]),
            TWO_PINS,
        );
        let value = Value::Fixed("IBIS".into());
        let error = convert(&symbol, "U", value, &mut fixture_resolver()).unwrap_err();
        assert!(
            error
                .message
                .starts_with("Sim.Device IBIS needs Sim.Library")
        );
    }
}
