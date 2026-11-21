//! Footprints (`.kicad_mod`).

use std::collections::BTreeSet;

use crate::{Error, naming::natural_cmp, sexpr};

/// The pads of a footprint that matter for a netlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Footprint {
    pub name: String,
    /// Numbers of the pads, in natural order. Pads without a number (paste apertures,
    /// non-plated holes) are left out; pads sharing a number appear once.
    pub pad_numbers: Vec<String>,
}

impl Footprint {
    pub fn parse(text: &str) -> Result<Self, Error> {
        let root = sexpr::parse(text).map_err(|e| Error::new(e.to_string()))?;
        // `module` is the head used before KiCad 6.
        if !matches!(root.head(), Some("footprint" | "module")) {
            return Err(Error::new("not a KiCad footprint (footprint)"));
        }
        let name = root.arg(0).unwrap_or_default().to_string();
        let numbers: BTreeSet<&str> = root
            .children("pad")
            .filter_map(|pad| pad.arg(0))
            .filter(|number| !number.is_empty())
            .collect();
        let mut pad_numbers: Vec<String> = numbers.into_iter().map(str::to_string).collect();
        pad_numbers.sort_by(|a, b| natural_cmp(a, b));
        Ok(Footprint { name, pad_numbers })
    }

    pub fn has_pad(&self, number: &str) -> bool {
        self.pad_numbers.iter().any(|pad| pad == number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pads() {
        let footprint = Footprint::parse(
            r#"(footprint "SOIC-8-1EP" (version 20241229)
                 (pad "1" smd roundrect (at 0 0) (size 1 1))
                 (pad "10" smd roundrect (at 0 0) (size 1 1))
                 (pad "2" smd roundrect (at 0 0) (size 1 1))
                 (pad "9" smd rect (at 0 0) (size 1 1))
                 (pad "9" thru_hole circle (at 0 0) (size 1 1))
                 (pad "" smd roundrect (at 0 0) (size 1 1)))"#,
        )
        .unwrap();
        assert_eq!(footprint.name, "SOIC-8-1EP");
        assert_eq!(footprint.pad_numbers, ["1", "2", "9", "10"]);
        assert!(footprint.has_pad("9"));
        assert!(!footprint.has_pad(""));
    }
}
