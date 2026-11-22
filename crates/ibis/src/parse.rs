//! Reads the keywords of an IBIS file into an [`IbisFile`].

use std::collections::BTreeSet;

use crate::{
    Component, Enable, Error, Header, IbisFile, IvPoint, IvTable, Model, ModelType, Package, Pin,
    PinModel, Polarity, Ramp, Skipped, Slope, TypMinMax, VtPoint, Waveform,
    lex::{Block, Line, lex},
    number::{parse_number, parse_optional},
};

/// Reads the text of an IBIS file.
///
/// # Errors
///
/// Fails on a syntax error, on a missing `[IBIS Ver]` or `Model_type`, on duplicates (keywords
/// that appear once, components or models with the same name) and on pins naming a model that
/// is not in the file.
pub fn parse(text: &str) -> Result<IbisFile, Error> {
    let (blocks, comment_char) = lex(text)?;
    let mut parser = Parser {
        names: blocks.iter().map(|b| b.name.as_str()).collect(),
        header: HeaderFields::default(),
        components: Vec::new(),
        models: Vec::new(),
        model_selectors: Vec::new(),
        skipped: Vec::new(),
        pin_refs: Vec::new(),
    };
    parser.run(&blocks)?;
    parser.finish(comment_char)
}

/// Where the keywords belong.
#[derive(Clone, Copy)]
enum Context {
    /// Before the first component or model.
    Top,
    Component(usize),
    Model(usize),
    /// In a scope that is skipped as a whole (`[Submodel]`, `[Model Selector]`, ...), whose
    /// keywords may have the names of model keywords.
    Skipped,
}

#[derive(Default)]
struct HeaderFields {
    ibis_ver: Option<String>,
    file_name: Option<String>,
    file_rev: Option<String>,
    date: Option<String>,
    source: Option<String>,
    notes: Option<String>,
    disclaimer: Option<String>,
    copyright: Option<String>,
}

struct Parser<'a> {
    /// The names of all keywords of the file.
    names: BTreeSet<&'a str>,
    header: HeaderFields,
    components: Vec<Component>,
    models: Vec<Model>,
    model_selectors: Vec<String>,
    skipped: Vec<Skipped>,
    /// The models named by pins, with the lines.
    pin_refs: Vec<(usize, String)>,
}

/// Top-level keywords whose scope, up to the next component or model, is skipped.
const SKIPPED_SCOPES: [&str; 4] = ["model selector", "submodel", "test data", "test load"];

impl<'a> Parser<'a> {
    fn run(&mut self, blocks: &'a [Block]) -> Result<(), Error> {
        let mut context = Context::Top;
        let mut i = 0;
        while let Some(b) = blocks.get(i) {
            i += 1;
            let name = b.name.as_str();
            match (name, context) {
                ("comment char", _) => {}
                ("ibis ver", _) => set_once(&mut self.header.ibis_ver, b, args(b)?)?,
                ("file name", _) => set_once(&mut self.header.file_name, b, args(b)?)?,
                ("file rev", _) => set_once(&mut self.header.file_rev, b, args(b)?)?,
                ("date", _) => set_once(&mut self.header.date, b, args(b)?)?,
                ("source", _) => set_once(&mut self.header.source, b, text(b))?,
                ("notes", _) => set_once(&mut self.header.notes, b, text(b))?,
                ("disclaimer", _) => set_once(&mut self.header.disclaimer, b, text(b))?,
                ("copyright", _) => set_once(&mut self.header.copyright, b, text(b))?,
                ("component", _) => {
                    let name = args(b)?;
                    if self.components.iter().any(|c| c.name == name) {
                        return Err(Error::new(b.line, format!("duplicate [Component] {name}")));
                    }
                    self.components.push(Component {
                        name,
                        manufacturer: None,
                        package: Package::default(),
                        pins: Vec::new(),
                    });
                    context = Context::Component(self.components.len() - 1);
                }
                ("model", _) => {
                    let model = self.model(b)?;
                    if self.models.iter().any(|m| m.name == model.name) {
                        return Err(Error::new(
                            b.line,
                            format!("duplicate [Model] {}", model.name),
                        ));
                    }
                    self.models.push(model);
                    context = Context::Model(self.models.len() - 1);
                }
                (_, _) if SKIPPED_SCOPES.contains(&name) => {
                    if name == "model selector" {
                        self.model_selectors.push(args(b)?);
                    }
                    self.skip(b);
                    context = Context::Skipped;
                }
                ("manufacturer" | "package" | "pin", Context::Component(c)) => {
                    self.component_keyword(c, b)?
                }
                ("manufacturer" | "package" | "pin", Context::Top | Context::Model(_)) => {
                    return Err(Error::new(
                        b.line,
                        format!("{} outside of a [Component]", b.keyword),
                    ));
                }
                (_, Context::Model(m)) if MODEL_KEYWORDS.contains(&name) => {
                    self.model_keyword(m, b)?
                }
                (_, Context::Top | Context::Component(_)) if MODEL_KEYWORDS.contains(&name) => {
                    return Err(Error::new(
                        b.line,
                        format!("{} outside of a [Model]", b.keyword),
                    ));
                }
                _ => {
                    self.skip(b);
                    // Skips a block with an end keyword (`[Define Package Model]` ...
                    // `[End Package Model]`) as a whole, since it may contain keywords of the
                    // component or the model.
                    if let Some(end) = end_keyword(name).filter(|e| self.names.contains(e.as_str()))
                        && let Some(n) = blocks[i..].iter().position(|b| b.name == end)
                    {
                        i += n + 1;
                    }
                }
            }
        }
        Ok(())
    }

    fn finish(self, comment_char: char) -> Result<IbisFile, Error> {
        let h = self.header;
        let ibis_ver = h
            .ibis_ver
            .ok_or_else(|| Error::new(1, "missing [IBIS Ver]"))?;
        for (line, name) in &self.pin_refs {
            if !self.models.iter().any(|m| &m.name == name) && !self.model_selectors.contains(name)
            {
                return Err(Error::new(
                    *line,
                    format!(
                        "the pin refers to `{name}`, which is not a [Model] or [Model Selector] of the file"
                    ),
                ));
            }
        }
        Ok(IbisFile {
            header: Header {
                ibis_ver,
                comment_char,
                file_name: h.file_name,
                file_rev: h.file_rev,
                date: h.date,
                source: h.source,
                notes: h.notes,
                disclaimer: h.disclaimer,
                copyright: h.copyright,
            },
            components: self.components,
            models: self.models,
            model_selectors: self.model_selectors,
            skipped: self.skipped,
        })
    }

    fn skip(&mut self, b: &Block) {
        self.skipped.push(Skipped {
            name: b.keyword.clone(),
            line: b.line,
        });
    }

    fn skip_subparam(&mut self, s: &Subparam) {
        self.skipped.push(Skipped {
            name: s.name.clone(),
            line: s.line,
        });
    }

    fn component_keyword(&mut self, c: usize, b: &Block) -> Result<(), Error> {
        match b.name.as_str() {
            "manufacturer" => {
                let manufacturer = args(b)?;
                set_once(&mut self.components[c].manufacturer, b, manufacturer)
            }
            "package" => {
                let mut package = Package::default();
                for s in subparams(&b.body) {
                    let field = match s.name.to_ascii_lowercase().as_str() {
                        "r_pkg" => &mut package.r_pkg,
                        "l_pkg" => &mut package.l_pkg,
                        "c_pkg" => &mut package.c_pkg,
                        _ => {
                            self.skip_subparam(&s);
                            continue;
                        }
                    };
                    let value = typ_min_max(&s.values, s.line, &s.name, parse_optional)?;
                    set_subparam(field, &s, value)?;
                }
                self.components[c].package = package;
                Ok(())
            }
            "pin" => self.pins(c, b),
            _ => unreachable!(),
        }
    }

    fn pins(&mut self, c: usize, b: &Block) -> Result<(), Error> {
        let mut pins = Vec::new();
        for l in &b.body {
            let tokens: Vec<_> = l.tokens().collect();
            let (name, signal, model, rlc) = match tokens[..] {
                [name, signal, model] => (name, signal, model, [None; 3]),
                [name, signal, model, r, l_, c_] => {
                    let mut rlc = [None; 3];
                    for (value, s) in rlc.iter_mut().zip([r, l_, c_]) {
                        *value = parse_optional(s).ok_or_else(|| {
                            Error::new(l.line, format!("invalid value `{s}` in [Pin]"))
                        })?;
                    }
                    (name, signal, model, rlc)
                }
                _ => {
                    return Err(Error::new(
                        l.line,
                        "a row of [Pin] needs a pin name, a signal name and a model name, \
                         optionally followed by R_pin, L_pin and C_pin",
                    ));
                }
            };
            if pins.iter().any(|p: &Pin| p.name == name) {
                return Err(Error::new(l.line, format!("duplicate pin {name}")));
            }
            let model = match model.to_ascii_uppercase().as_str() {
                "POWER" => PinModel::Power,
                "GND" => PinModel::Gnd,
                "NC" => PinModel::Nc,
                _ => {
                    self.pin_refs.push((l.line, model.to_string()));
                    PinModel::Named(model.to_string())
                }
            };
            pins.push(Pin {
                name: name.to_string(),
                signal_name: signal.to_string(),
                model,
                r_pin: rlc[0],
                l_pin: rlc[1],
                c_pin: rlc[2],
            });
        }
        let component = &mut self.components[c];
        if !component.pins.is_empty() {
            return Err(Error::new(b.line, "duplicate [Pin]"));
        }
        component.pins = pins;
        Ok(())
    }

    /// A model from `[Model]` and its subparameters; the other keywords are added later.
    fn model(&mut self, b: &Block) -> Result<Model, Error> {
        let mut model_type = None;
        let mut model = Model {
            name: args(b)?,
            model_type: ModelType::Input,
            polarity: None,
            enable: None,
            vinl: None,
            vinh: None,
            vmeas: None,
            vref: None,
            cref: None,
            rref: None,
            c_comp: None,
            temperature_range: None,
            voltage_range: None,
            pullup_reference: None,
            pulldown_reference: None,
            power_clamp_reference: None,
            gnd_clamp_reference: None,
            pullup: None,
            pulldown: None,
            gnd_clamp: None,
            power_clamp: None,
            ramp: None,
            rising_waveforms: Vec::new(),
            falling_waveforms: Vec::new(),
        };
        for s in subparams(&b.body) {
            let lower = s.name.to_ascii_lowercase();
            match lower.as_str() {
                "model_type" => {
                    let value = s.single()?;
                    let t = ModelType::from_name(value).ok_or_else(|| {
                        Error::new(s.line, format!("unknown Model_type `{value}`"))
                    })?;
                    set_subparam(&mut model_type, &s, t)?;
                }
                "polarity" => {
                    let p = match s.single()?.to_ascii_lowercase().as_str() {
                        "non-inverting" => Polarity::NonInverting,
                        "inverting" => Polarity::Inverting,
                        other => {
                            return Err(Error::new(s.line, format!("unknown Polarity `{other}`")));
                        }
                    };
                    set_subparam(&mut model.polarity, &s, p)?;
                }
                "enable" => {
                    let e = match s.single()?.to_ascii_lowercase().as_str() {
                        "active-high" => Enable::ActiveHigh,
                        "active-low" => Enable::ActiveLow,
                        other => {
                            return Err(Error::new(s.line, format!("unknown Enable `{other}`")));
                        }
                    };
                    set_subparam(&mut model.enable, &s, e)?;
                }
                "vinl" | "vinh" | "vmeas" | "vref" | "cref" | "rref" => {
                    let field = match lower.as_str() {
                        "vinl" => &mut model.vinl,
                        "vinh" => &mut model.vinh,
                        "vmeas" => &mut model.vmeas,
                        "vref" => &mut model.vref,
                        "cref" => &mut model.cref,
                        _ => &mut model.rref,
                    };
                    let value = s.single()?;
                    let v = parse_number(value).ok_or_else(|| s.invalid(value))?;
                    set_subparam(field, &s, v)?;
                }
                "c_comp" => {
                    let value = typ_min_max(&s.values, s.line, &s.name, parse_optional)?;
                    set_subparam(&mut model.c_comp, &s, value)?;
                }
                _ => self.skip_subparam(&s),
            }
        }
        model.model_type = model_type.ok_or_else(|| {
            Error::new(b.line, format!("[Model] {} has no Model_type", model.name))
        })?;
        Ok(model)
    }

    fn model_keyword(&mut self, m: usize, b: &Block) -> Result<(), Error> {
        let name = b.name.as_str();
        match name {
            "pullup" | "pulldown" | "gnd clamp" | "power clamp" => {
                let table = iv_table(b)?;
                let model = &mut self.models[m];
                let field = match name {
                    "pullup" => &mut model.pullup,
                    "pulldown" => &mut model.pulldown,
                    "gnd clamp" => &mut model.gnd_clamp,
                    _ => &mut model.power_clamp,
                };
                set_once(field, b, table)
            }
            "ramp" => {
                let ramp = self.ramp(b)?;
                set_once(&mut self.models[m].ramp, b, ramp)
            }
            "rising waveform" | "falling waveform" => {
                let waveform = self.waveform(b)?;
                let model = &mut self.models[m];
                match name {
                    "rising waveform" => model.rising_waveforms.push(waveform),
                    _ => model.falling_waveforms.push(waveform),
                }
                Ok(())
            }
            _ => {
                // The ranges and references: three columns after the keyword.
                let values: Vec<_> = b
                    .args
                    .split_whitespace()
                    .chain(b.body.iter().flat_map(Line::tokens))
                    .collect();
                let value = typ_min_max(&values, b.line, &b.keyword, parse_optional)?;
                let model = &mut self.models[m];
                let field = match name {
                    "temperature range" => &mut model.temperature_range,
                    "voltage range" => &mut model.voltage_range,
                    "pullup reference" => &mut model.pullup_reference,
                    "pulldown reference" => &mut model.pulldown_reference,
                    "power clamp reference" => &mut model.power_clamp_reference,
                    "gnd clamp reference" => &mut model.gnd_clamp_reference,
                    _ => unreachable!(),
                };
                set_once(field, b, value)
            }
        }
    }

    fn ramp(&mut self, b: &Block) -> Result<Ramp, Error> {
        let (mut rise, mut fall, mut r_load) = (None, None, None);
        for s in subparams(&b.body) {
            match s.name.to_ascii_lowercase().as_str() {
                "dv/dt_r" => {
                    let value = typ_min_max(&s.values, s.line, &s.name, parse_slope)?;
                    set_subparam(&mut rise, &s, value)?;
                }
                "dv/dt_f" => {
                    let value = typ_min_max(&s.values, s.line, &s.name, parse_slope)?;
                    set_subparam(&mut fall, &s, value)?;
                }
                "r_load" => {
                    let value = s.single()?;
                    let r = parse_number(value).ok_or_else(|| s.invalid(value))?;
                    set_subparam(&mut r_load, &s, r)?;
                }
                _ => self.skip_subparam(&s),
            }
        }
        match (rise, fall) {
            (Some(dv_dt_r), Some(dv_dt_f)) => Ok(Ramp {
                dv_dt_r,
                dv_dt_f,
                r_load,
            }),
            _ => Err(Error::new(b.line, "[Ramp] needs both dV/dt_r and dV/dt_f")),
        }
    }
}

impl Parser<'_> {
    /// A `[Rising Waveform]` or `[Falling Waveform]`: the subparameters of the fixture, then the
    /// rows of the table (lines starting with a number).
    fn waveform(&mut self, b: &Block) -> Result<Waveform, Error> {
        let (rows, others): (Vec<&Line>, Vec<&Line>) = b
            .body
            .iter()
            .partition(|l| l.tokens().next().is_some_and(|t| parse_number(t).is_some()));
        let mut r_fixture = None;
        let (mut v_fixture, mut v_fixture_min, mut v_fixture_max) = (None, None, None);
        let mut optional = [None; 5];
        for s in subparams(others) {
            let lower = s.name.to_ascii_lowercase();
            let field = match lower.as_str() {
                "r_fixture" => &mut r_fixture,
                "v_fixture" => &mut v_fixture,
                "v_fixture_min" => &mut v_fixture_min,
                "v_fixture_max" => &mut v_fixture_max,
                "l_fixture" => &mut optional[0],
                "c_fixture" => &mut optional[1],
                "r_dut" => &mut optional[2],
                "l_dut" => &mut optional[3],
                "c_dut" => &mut optional[4],
                _ => {
                    self.skip_subparam(&s);
                    continue;
                }
            };
            let value = s.single()?;
            let v = parse_number(value).ok_or_else(|| s.invalid(value))?;
            set_subparam(field, &s, v)?;
        }
        let missing = |what: &str| Error::new(b.line, format!("{} needs {what}", b.keyword));
        let r_fixture = r_fixture.ok_or_else(|| missing("R_fixture"))?;
        let v_fixture = v_fixture.ok_or_else(|| missing("V_fixture"))?;
        let points = rows
            .iter()
            .map(|l| {
                let tokens: Vec<_> = l.tokens().collect();
                let (time, voltage) = tokens.split_first().unwrap();
                Ok(VtPoint {
                    time: parse_number(time).unwrap(),
                    voltage: typ_min_max(voltage, l.line, &b.keyword, parse_optional)?,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        if points.len() < 2 {
            return Err(missing("at least two rows"));
        }
        if let Some(w) = points.windows(2).find(|w| w[1].time <= w[0].time) {
            return Err(Error::new(
                b.line,
                format!(
                    "the times of {} must increase: {} after {}",
                    b.keyword, w[1].time, w[0].time
                ),
            ));
        }
        let [l_fixture, c_fixture, r_dut, l_dut, c_dut] = optional;
        Ok(Waveform {
            r_fixture,
            v_fixture: TypMinMax {
                typ: v_fixture,
                min: v_fixture_min,
                max: v_fixture_max,
            },
            l_fixture,
            c_fixture,
            r_dut,
            l_dut,
            c_dut,
            points,
        })
    }
}

/// The keywords read in the scope of a `[Model]`, besides `[Model]` itself.
const MODEL_KEYWORDS: [&str; 13] = [
    "temperature range",
    "voltage range",
    "pullup reference",
    "pulldown reference",
    "power clamp reference",
    "gnd clamp reference",
    "pullup",
    "pulldown",
    "gnd clamp",
    "power clamp",
    "ramp",
    "rising waveform",
    "falling waveform",
];

/// The keyword that ends a block, if the keyword may start one: `[End <name>]`, with the
/// exceptions of the specification (`[Begin Board Description]` ends with
/// `[End Board Description]`, `[Define Package Model]` with `[End Package Model]`).
fn end_keyword(name: &str) -> Option<String> {
    if name.starts_with("end ") {
        return None;
    }
    let body = name
        .strip_prefix("begin ")
        .or_else(|| name.strip_prefix("define "))
        .unwrap_or(name);
    Some(format!("end {body}"))
}

/// A line of subparameters: a name and its values (`Vinl = 0.8V`, `C_comp 1p NA NA`).
struct Subparam {
    line: usize,
    name: String,
    values: Vec<String>,
}

impl Subparam {
    fn single(&self) -> Result<&str, Error> {
        match &self.values[..] {
            [value] => Ok(value),
            _ => Err(Error::new(
                self.line,
                format!("{} needs one value", self.name),
            )),
        }
    }

    fn invalid(&self, value: &str) -> Error {
        Error::new(
            self.line,
            format!("invalid value `{value}` for {}", self.name),
        )
    }
}

fn subparams<'l>(lines: impl IntoIterator<Item = &'l Line>) -> Vec<Subparam> {
    lines
        .into_iter()
        .map(|l| {
            let text = l.text.replace('=', " = ");
            let mut tokens = text.split_whitespace().map(str::to_string);
            let name = tokens.next().unwrap_or_default();
            let mut values: Vec<_> = tokens.collect();
            if values.first().is_some_and(|v| v == "=") {
                values.remove(0);
            }
            Subparam {
                line: l.line,
                name,
                values,
            }
        })
        .collect()
}

/// The text after the keyword, which must not be empty.
fn args(b: &Block) -> Result<String, Error> {
    if b.args.is_empty() {
        Err(Error::new(b.line, format!("{} needs a value", b.keyword)))
    } else {
        Ok(b.args.clone())
    }
}

/// The text after the keyword and the following lines, joined by `\n`.
fn text(b: &Block) -> String {
    std::iter::once(b.args.as_str())
        .filter(|a| !a.is_empty())
        .chain(b.body.iter().map(|l| l.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn set_once<T>(field: &mut Option<T>, b: &Block, value: T) -> Result<(), Error> {
    if field.is_some() {
        return Err(Error::new(b.line, format!("duplicate {}", b.keyword)));
    }
    *field = Some(value);
    Ok(())
}

fn set_subparam<T>(field: &mut Option<T>, s: &Subparam, value: T) -> Result<(), Error> {
    if field.is_some() {
        return Err(Error::new(s.line, format!("duplicate {}", s.name)));
    }
    *field = Some(value);
    Ok(())
}

/// The typical value, optionally followed by the minimum and the maximum (which may be `NA`).
fn typ_min_max<T, S: AsRef<str>>(
    values: &[S],
    line: usize,
    what: &str,
    parse: impl Fn(&str) -> Option<Option<T>>,
) -> Result<TypMinMax<T>, Error> {
    let value = |s: &S| {
        let s = s.as_ref();
        parse(s).ok_or_else(|| Error::new(line, format!("invalid value `{s}` for {what}")))
    };
    let (typ, min, max) = match values {
        [typ] => (value(typ)?, None, None),
        [typ, min, max] => (value(typ)?, value(min)?, value(max)?),
        _ => {
            return Err(Error::new(
                line,
                format!("{what} needs the typical, minimum and maximum values"),
            ));
        }
    };
    let typ = typ.ok_or_else(|| Error::new(line, format!("the typical value of {what} is NA")))?;
    Ok(TypMinMax { typ, min, max })
}

/// A `dV/dt` value (`3.0/30n`) or `NA`.
fn parse_slope(s: &str) -> Option<Option<Slope>> {
    if s.eq_ignore_ascii_case("NA") {
        return Some(None);
    }
    let (dv, dt) = s.split_once('/')?;
    Some(Some(Slope {
        dv: parse_number(dv)?,
        dt: parse_number(dt)?,
    }))
}

fn iv_table(b: &Block) -> Result<IvTable, Error> {
    let points = b
        .body
        .iter()
        .map(|l| {
            let tokens: Vec<_> = l.tokens().collect();
            let (voltage, current) = tokens
                .split_first()
                .ok_or_else(|| Error::new(l.line, "empty row"))?;
            Ok(IvPoint {
                voltage: parse_number(voltage).ok_or_else(|| {
                    Error::new(
                        l.line,
                        format!("invalid voltage `{voltage}` in {}", b.keyword),
                    )
                })?,
                current: typ_min_max(current, l.line, &b.keyword, parse_optional)?,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    if points.is_empty() {
        return Err(Error::new(b.line, format!("{} has no rows", b.keyword)));
    }
    Ok(IvTable { points })
}
