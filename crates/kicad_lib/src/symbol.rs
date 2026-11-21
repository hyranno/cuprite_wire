//! Symbols of a KiCad symbol library (`.kicad_sym`).

use std::collections::BTreeMap;

use crate::{
    Error,
    naming::natural_cmp,
    sexpr::{self, Sexpr},
};

/// Electrical type of a pin, as written in the library (`passive`, `power_in`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ElectricalType {
    Input,
    Output,
    Bidirectional,
    TriState,
    Passive,
    Free,
    Unspecified,
    PowerIn,
    PowerOut,
    OpenCollector,
    OpenEmitter,
    NoConnect,
}

impl ElectricalType {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "input" => ElectricalType::Input,
            "output" => ElectricalType::Output,
            "bidirectional" => ElectricalType::Bidirectional,
            "tri_state" => ElectricalType::TriState,
            "passive" => ElectricalType::Passive,
            "free" => ElectricalType::Free,
            "unspecified" => ElectricalType::Unspecified,
            "power_in" => ElectricalType::PowerIn,
            "power_out" => ElectricalType::PowerOut,
            "open_collector" => ElectricalType::OpenCollector,
            "open_emitter" => ElectricalType::OpenEmitter,
            "no_connect" => ElectricalType::NoConnect,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ElectricalType::Input => "input",
            ElectricalType::Output => "output",
            ElectricalType::Bidirectional => "bidirectional",
            ElectricalType::TriState => "tri_state",
            ElectricalType::Passive => "passive",
            ElectricalType::Free => "free",
            ElectricalType::Unspecified => "unspecified",
            ElectricalType::PowerIn => "power_in",
            ElectricalType::PowerOut => "power_out",
            ElectricalType::OpenCollector => "open_collector",
            ElectricalType::OpenEmitter => "open_emitter",
            ElectricalType::NoConnect => "no_connect",
        }
    }
}

/// A pin of a symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    /// The pin number as written in the library. It is the footprint pad number, unless it is
    /// KiCad 10's stacked pin notation (`[1,3]`, `[1-4]`), which stands for several pads.
    pub number: String,
    /// The footprint pad numbers of the pin: `number` itself, or the pads of the stacked pin
    /// notation (see [`expand_stacked_pin_number`]).
    pub pads: Vec<String>,
    /// The pin name; `None` when the library has no name (`~` or empty).
    pub name: Option<String>,
    pub electrical_type: ElectricalType,
    /// The unit the pin belongs to; 0 for pins shared by all units.
    pub unit: u32,
    /// Position of the pin's connection point in the symbol (library coordinates).
    pub at: (f64, f64),
    pub hidden: bool,
}

/// A symbol with its `extends` resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub name: String,
    /// Properties in the order of the library (`Reference`, `Value`, `Footprint`, `Sim.*`, ...).
    pub properties: Vec<(String, String)>,
    /// Pins sorted by number (in natural order: `2` before `10`).
    pub pins: Vec<Pin>,
    pub unit_count: u32,
    pub power: bool,
    pub exclude_from_sim: bool,
    pub in_bom: bool,
    pub on_board: bool,
}

impl Symbol {
    pub fn property(&self, name: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Groups of pin numbers placed at the same position in the same unit. KiCad connects such
    /// stacked pins in the schematic, so they are connected inside the part.
    /// (These are pin numbers; the pads of a stacked pin notation are not listed here.)
    pub fn stacked_pins(&self) -> Vec<Vec<String>> {
        let mut groups: BTreeMap<(u32, i64, i64), Vec<String>> = BTreeMap::new();
        for pin in &self.pins {
            // Library coordinates are in mm with at most 4 decimals.
            let key = (
                pin.unit,
                (pin.at.0 * 1e4).round() as i64,
                (pin.at.1 * 1e4).round() as i64,
            );
            groups.entry(key).or_default().push(pin.number.clone());
        }
        let mut groups: Vec<_> = groups
            .into_values()
            .filter(|group| group.len() > 1)
            .collect();
        groups.sort_by(|a, b| natural_cmp(&a[0], &b[0]));
        groups
    }
}

/// A parsed symbol library.
#[derive(Debug, Clone)]
pub struct SymbolLib {
    symbols: BTreeMap<String, Sexpr>,
}

impl SymbolLib {
    pub fn parse(text: &str) -> Result<Self, Error> {
        let root = sexpr::parse(text).map_err(|e| Error::new(e.to_string()))?;
        if root.head() != Some("kicad_symbol_lib") {
            return Err(Error::new("not a KiCad symbol library (kicad_symbol_lib)"));
        }
        let mut symbols = BTreeMap::new();
        for symbol in root.children("symbol") {
            let name = symbol
                .arg(0)
                .ok_or_else(|| Error::new(format!("{}: symbol without a name", symbol.pos())))?;
            symbols.insert(name.to_string(), symbol.clone());
        }
        Ok(SymbolLib { symbols })
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.symbols.keys().map(String::as_str)
    }

    /// The symbol `name`, with properties and flags of its `extends` chain resolved.
    pub fn symbol(&self, name: &str) -> Result<Symbol, Error> {
        let mut chain = vec![self.raw(name)?];
        while let Some(parent) = chain.last().unwrap().child("extends") {
            let parent_name = parent
                .arg(0)
                .ok_or_else(|| Error::new(format!("{}: extends without a name", parent.pos())))?;
            if chain.len() > 16 {
                return Err(Error::new(format!(
                    "symbol {name:?}: extends chain is too long"
                )));
            }
            chain.push(self.raw(parent_name)?);
        }
        let base = *chain.last().unwrap();
        let base_name = base.arg(0).unwrap();

        // Properties: from the base, overridden from parent to child.
        let mut properties: Vec<(String, String)> = Vec::new();
        for symbol in chain.iter().rev() {
            for property in symbol.children("property") {
                let (Some(key), Some(value)) = (property.arg(0), property.arg(1)) else {
                    continue;
                };
                match properties.iter_mut().find(|(k, _)| k == key) {
                    Some(entry) => entry.1 = value.to_string(),
                    None => properties.push((key.to_string(), value.to_string())),
                }
            }
        }
        // Flags: the nearest symbol in the chain that has them.
        let flag = |name: &str, default: bool| {
            chain
                .iter()
                .find_map(|symbol| symbol.flag(name))
                .unwrap_or(default)
        };

        let pins = collect_pins(base, base_name)?;
        let unit_count = pins.iter().map(|pin| pin.unit).max().unwrap_or(0).max(1);
        Ok(Symbol {
            name: name.to_string(),
            properties,
            pins,
            unit_count,
            power: chain.iter().any(|symbol| symbol.child("power").is_some()),
            exclude_from_sim: flag("exclude_from_sim", false),
            in_bom: flag("in_bom", true),
            on_board: flag("on_board", true),
        })
    }

    fn raw(&self, name: &str) -> Result<&Sexpr, Error> {
        self.symbols
            .get(name)
            .ok_or_else(|| Error::new(format!("symbol {name:?} not found in the library")))
    }
}

/// Collects the pins of the unit sub-symbols `<base>_<unit>_<style>`. Only body style 1
/// (and 0, shared by all styles) is used: other styles (De Morgan) repeat the same pins.
fn collect_pins(base: &Sexpr, base_name: &str) -> Result<Vec<Pin>, Error> {
    let mut pins: Vec<Pin> = Vec::new();
    for unit_symbol in base.children("symbol") {
        let unit_name = unit_symbol.arg(0).unwrap_or_default();
        let (unit, style) = unit_name
            .strip_prefix(base_name)
            .and_then(|rest| rest.strip_prefix('_'))
            .and_then(|rest| rest.split_once('_'))
            .and_then(|(unit, style)| Some((unit.parse::<u32>().ok()?, style.parse::<u32>().ok()?)))
            .ok_or_else(|| {
                Error::new(format!(
                    "{}: unexpected unit symbol name {unit_name:?}",
                    unit_symbol.pos()
                ))
            })?;
        if style > 1 {
            continue;
        }
        for pin in unit_symbol.children("pin") {
            let pin = parse_pin(pin, unit)?;
            match pins.iter().find(|other| other.number == pin.number) {
                None => pins.push(pin),
                // The same number in several units (e.g. a shared supply pin): one pin.
                Some(other)
                    if other.electrical_type == pin.electrical_type && other.name == pin.name => {}
                Some(other) => {
                    return Err(Error::new(format!(
                        "pin {:?} is defined twice with different names or types ({:?} {} / {:?} {})",
                        pin.number,
                        other.name,
                        other.electrical_type.as_str(),
                        pin.name,
                        pin.electrical_type.as_str()
                    )));
                }
            }
        }
    }
    let mut pin_of_pad: BTreeMap<&str, &str> = BTreeMap::new();
    for pin in &pins {
        for pad in &pin.pads {
            if let Some(other) = pin_of_pad.insert(pad, &pin.number) {
                return Err(Error::new(format!(
                    "pad {pad:?} belongs to two pins ({other:?} and {:?})",
                    pin.number
                )));
            }
        }
    }
    pins.sort_by(|a, b| natural_cmp(&a.number, &b.number));
    Ok(pins)
}

fn parse_pin(pin: &Sexpr, unit: u32) -> Result<Pin, Error> {
    let error = |message: &str| Error::new(format!("{}: pin {message}", pin.pos()));
    let electrical_type = pin
        .arg(0)
        .and_then(ElectricalType::parse)
        .ok_or_else(|| error("without a valid electrical type"))?;
    let number = pin
        .child("number")
        .and_then(|n| n.arg(0))
        .ok_or_else(|| error("without a number"))?;
    if number.is_empty() {
        return Err(error("with an empty number"));
    }
    let name = pin
        .child("name")
        .and_then(|n| n.arg(0))
        .filter(|name| !name.is_empty() && *name != "~")
        .map(str::to_string);
    let at = pin.child("at").ok_or_else(|| error("without a position"))?;
    let coordinate = |i| {
        at.arg(i)
            .and_then(|v| v.parse::<f64>().ok())
            .ok_or_else(|| error("with an invalid position"))
    };
    let pads = expand_stacked_pin_number(number).map_err(|e| e.context(pin.pos()))?;
    Ok(Pin {
        number: number.to_string(),
        pads,
        name,
        electrical_type,
        unit,
        at: (coordinate(0)?, coordinate(1)?),
        hidden: pin.flag("hide").unwrap_or(false),
    })
}

/// The pad numbers of a pin number: the number itself, or the pads of KiCad 10's stacked pin
/// notation, `[` and `]` around comma-separated pad numbers or ranges (`[1,3]`, `[1-4]`,
/// `[A1-A4,B2]`; the ends of a range share the prefix before their trailing digits). Expanded as
/// KiCad's `ExpandStackedPinNotation` does; KiCad keeps an invalid notation as a single pad number
/// (with an ERC warning), while this is an error.
pub fn expand_stacked_pin_number(number: &str) -> Result<Vec<String>, Error> {
    if !number.contains(['[', ']']) {
        return Ok(vec![number.to_string()]);
    }
    let invalid = || Error::new(format!("invalid stacked pin number {number:?}"));
    let inner = number
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .ok_or_else(invalid)?;
    let mut pads = Vec::new();
    for part in inner
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let Some((start, end)) = part.split_once('-') else {
            pads.push(part.to_string());
            continue;
        };
        let (prefix, start) = split_trailing_number(start.trim()).ok_or_else(invalid)?;
        let (end_prefix, end) = split_trailing_number(end.trim()).ok_or_else(invalid)?;
        if prefix != end_prefix || start > end {
            return Err(invalid());
        }
        pads.extend((start..=end).map(|n| format!("{prefix}{n}")));
    }
    if pads.is_empty() {
        return Err(invalid());
    }
    Ok(pads)
}

/// Splits `A12` into `("A", 12)`; `None` without trailing digits.
fn split_trailing_number(text: &str) -> Option<(&str, u64)> {
    let prefix = text.trim_end_matches(|c: char| c.is_ascii_digit());
    let number = text[prefix.len()..].parse().ok()?;
    Some((prefix, number))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::strings;

    const LIB: &str = r#"(kicad_symbol_lib (version 20241209) (generator "test")
  (symbol "AMP"
    (exclude_from_sim no) (in_bom yes) (on_board yes)
    (property "Reference" "U" (at 0 0 0))
    (property "Value" "AMP" (at 0 0 0))
    (property "Description" "base" (at 0 0 0))
    (symbol "AMP_0_1" (rectangle (start 0 0) (end 1 1)))
    (symbol "AMP_1_1"
      (pin output line (at 5 0 180) (length 2) (name "~" (effects)) (number "1" (effects)))
      (pin input line (at -5 1 0) (length 2) (name "+" (effects)) (number "3" (effects)))
      (pin input line (at -5 -1 0) (length 2) (name "-" (effects)) (number "2" (effects))))
    (symbol "AMP_1_2"
      (pin output line (at 5 0 180) (length 2) (name "~" (effects)) (number "1" (effects))))
    (symbol "AMP_2_1"
      (pin power_in line (at 0 5 270) (length 2) (name "V+" (effects)) (number "10" (effects)))
      (pin power_in line (at 0 -5 90) (length 2) (name "V-" (effects)) (number "4" (effects)))
      (pin passive line (at 0 -5 90) (length 2) (hide yes) (name "V-" (effects)) (number "5" (effects)))))
  (symbol "AMP2"
    (extends "AMP")
    (in_bom no)
    (property "Value" "AMP2" (at 0 0 0))
    (property "Sim.Device" "SUBCKT" (at 0 0 0))))
"#;

    #[test]
    fn test_symbol() {
        let lib = SymbolLib::parse(LIB).unwrap();
        let symbol = lib.symbol("AMP").unwrap();
        let numbers: Vec<_> = symbol.pins.iter().map(|pin| pin.number.as_str()).collect();
        assert_eq!(numbers, ["1", "2", "3", "4", "5", "10"]);
        assert_eq!(symbol.unit_count, 2);
        assert_eq!(symbol.pins[0].name, None);
        assert_eq!(symbol.pins[2].name.as_deref(), Some("+"));
        assert_eq!(symbol.pins[5].electrical_type, ElectricalType::PowerIn);
        assert!(symbol.pins[4].hidden);
        assert_eq!(
            symbol.stacked_pins(),
            vec![vec!["4".to_string(), "5".to_string()]]
        );
        assert!(symbol.in_bom);
    }

    #[test]
    fn test_extends() {
        let lib = SymbolLib::parse(LIB).unwrap();
        let symbol = lib.symbol("AMP2").unwrap();
        assert_eq!(symbol.property("Value"), Some("AMP2"));
        assert_eq!(symbol.property("Description"), Some("base"));
        assert_eq!(symbol.property("Sim.Device"), Some("SUBCKT"));
        assert_eq!(symbol.pins.len(), 6);
        assert!(!symbol.in_bom);
        assert!(lib.symbol("NONE").is_err());
    }

    #[test]
    fn test_expand_stacked_pin_number() {
        let expand = |number| expand_stacked_pin_number(number).map_err(|e| e.message);
        assert_eq!(expand("1"), Ok(strings(&["1"])));
        assert_eq!(expand("[2,13]"), Ok(strings(&["2", "13"])));
        assert_eq!(expand("[1-4]"), Ok(strings(&["1", "2", "3", "4"])));
        assert_eq!(
            expand("[ A1 - A3 , B2,]"),
            Ok(strings(&["A1", "A2", "A3", "B2"]))
        );
        assert_eq!(expand("[AA9-AA10]"), Ok(strings(&["AA9", "AA10"])));
        assert_eq!(expand("[7]"), Ok(strings(&["7"])));
        for invalid in [
            "[1,2", "1]", "x[1]", "[]", "[ , ]", "[A1-B2]", "[4-1]", "[A-C]",
        ] {
            assert_eq!(
                expand(invalid),
                Err(format!("invalid stacked pin number {invalid:?}"))
            );
        }
    }

    #[test]
    fn test_stacked_pin_notation() {
        let lib = SymbolLib::parse(
            r#"(kicad_symbol_lib (symbol "X"
                 (symbol "X_1_1"
                   (pin power_in line (at 0 0 0) (name "GND") (number "[2,4-5]"))
                   (pin input line (at 0 1 0) (name "A") (number "1")))))"#,
        )
        .unwrap();
        let symbol = lib.symbol("X").unwrap();
        assert_eq!(symbol.pins[0].number, "1");
        assert_eq!(symbol.pins[1].number, "[2,4-5]");
        assert_eq!(symbol.pins[1].pads, strings(&["2", "4", "5"]));
        let lib = SymbolLib::parse(
            r#"(kicad_symbol_lib (symbol "X"
                 (symbol "X_1_1" (pin input line (at 0 0 0) (name "A") (number "[1")))))"#,
        )
        .unwrap();
        let message = lib.symbol("X").unwrap_err().message;
        assert!(
            message.ends_with("invalid stacked pin number \"[1\""),
            "{message}"
        );
    }

    #[test]
    fn test_conflicting_pins() {
        let lib = SymbolLib::parse(
            r#"(kicad_symbol_lib (symbol "X"
                 (symbol "X_1_1" (pin input line (at 0 0 0) (name "A") (number "1")))
                 (symbol "X_2_1" (pin output line (at 0 0 0) (name "A") (number "1")))))"#,
        )
        .unwrap();
        assert!(
            lib.symbol("X")
                .unwrap_err()
                .to_string()
                .contains("defined twice")
        );
    }
}
