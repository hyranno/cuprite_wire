//! Bill of materials (BOM) of a KiCad netlist, written as CSV like `kicad-cli sch export bom`.
//!
//! See the guide [Exporting a KiCad netlist](https://github.com/hyranno/cuprite_wire/blob/main/doc/kicad_netlist_usage.md).

use std::{collections::BTreeMap, fmt, io};

use crate::{
    kicad::{KicadNetlist, netlist::KicadComponent},
    naming::natural_cmp,
};

/// What a BOM column shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BomField {
    /// The references of the row (`R1,R3-R5`).
    Reference,
    /// The value (empty if the part has none).
    Value,
    /// The footprint (`Library:Name`).
    Footprint,
    /// The symbol (`Library:Name`, empty if not set).
    LibSource,
    /// The number of components in the row.
    Quantity,
    /// `DNP` if the components are not populated, empty otherwise.
    Dnp,
    /// A field of the part (`KicadPartInfo::fields`, e.g. `MPN`), empty if the part has none.
    Field(String),
}

/// A column of a BOM: what it shows and its label in the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BomColumn {
    /// What the column shows.
    pub field: BomField,
    /// The label written in the header.
    pub label: String,
}

impl BomColumn {
    /// A column showing `field` labeled `label`.
    pub fn new(field: BomField, label: impl Into<String>) -> Self {
        BomColumn {
            field,
            label: label.into(),
        }
    }
}

/// How to make a BOM. The default is the default of `kicad-cli sch export bom`, with the components
/// grouped: the columns `Refs`, `Value`, `Footprint`, `Qty` and `DNP`, and reference ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BomOptions {
    /// The columns, in order.
    pub columns: Vec<BomColumn>,
    /// Groups the components whose cells are all equal (except the references and the quantity)
    /// into one row. Populated and not populated (DNP) components are never in the same row.
    pub group: bool,
    /// Leaves out the components that are not populated (DNP).
    pub exclude_dnp: bool,
    /// Writes three or more consecutive references as a range (`R3-R5`).
    pub ref_ranges: bool,
}

impl Default for BomOptions {
    fn default() -> Self {
        BomOptions {
            columns: vec![
                BomColumn::new(BomField::Reference, "Refs"),
                BomColumn::new(BomField::Value, "Value"),
                BomColumn::new(BomField::Footprint, "Footprint"),
                BomColumn::new(BomField::Quantity, "Qty"),
                BomColumn::new(BomField::Dnp, "DNP"),
            ],
            group: true,
            exclude_dnp: false,
            ref_ranges: true,
        }
    }
}

impl BomOptions {
    /// Appends a column showing the part field `name`, labeled with its name.
    pub fn with_field(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.columns
            .push(BomColumn::new(BomField::Field(name.clone()), name));
        self
    }
}

/// A bill of materials. Write it as CSV with [`KicadBom::write`] or `Display`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KicadBom {
    columns: Vec<BomColumn>,
    rows: Vec<BomRow>,
}

/// A row of a BOM: components with the same cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BomRow {
    /// The references of the components, in natural order (`R2` before `R10`).
    pub references: Vec<String>,
    /// Whether the components are not populated (DNP).
    pub dnp: bool,
    /// The cells, one per column.
    pub cells: Vec<String>,
}

impl KicadNetlist {
    /// The bill of materials of the netlist. Components excluded from the BOM
    /// (`PartFlags::exclude_from_bom`) are left out; components not on the board are not.
    ///
    /// Rows are sorted by their first reference in natural order.
    pub fn bom(&self, options: &BomOptions) -> KicadBom {
        let components = self
            .components
            .iter()
            .filter(|c| !c.flags.exclude_from_bom && !(options.exclude_dnp && c.flags.dnp));
        // The cells that do not depend on the group, `None` for the references and the quantity.
        let key = |c: &KicadComponent| -> (bool, Vec<Option<String>>) {
            let cells = options
                .columns
                .iter()
                .map(|column| component_cell(c, &column.field))
                .collect();
            (c.flags.dnp, cells)
        };
        let mut groups: Vec<Vec<&KicadComponent>> = if options.group {
            let mut groups: BTreeMap<_, Vec<_>> = BTreeMap::new();
            for c in components {
                groups.entry(key(c)).or_default().push(c);
            }
            groups.into_values().collect()
        } else {
            components.map(|c| vec![c]).collect()
        };
        for group in &mut groups {
            group.sort_by(|a, b| natural_cmp(&a.ref_name, &b.ref_name));
        }
        groups.sort_by(|a, b| natural_cmp(&a[0].ref_name, &b[0].ref_name));

        let rows = groups
            .into_iter()
            .map(|group| {
                let references: Vec<String> = group.iter().map(|c| c.ref_name.clone()).collect();
                let cells = options
                    .columns
                    .iter()
                    .map(|column| match &column.field {
                        BomField::Reference => shorthand(&references, options.ref_ranges),
                        BomField::Quantity => references.len().to_string(),
                        field => component_cell(group[0], field).unwrap_or_default(),
                    })
                    .collect();
                BomRow {
                    references,
                    dnp: group[0].flags.dnp,
                    cells,
                }
            })
            .collect();
        KicadBom {
            columns: options.columns.clone(),
            rows,
        }
    }
}

/// The cell of a component, `None` for the cells of the whole row.
fn component_cell(c: &KicadComponent, field: &BomField) -> Option<String> {
    Some(match field {
        BomField::Reference | BomField::Quantity => return None,
        BomField::Value => c.value.clone(),
        BomField::Footprint => c.footprint.to_string(),
        BomField::LibSource => c
            .lib_source
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        BomField::Dnp => if c.flags.dnp { "DNP" } else { "" }.to_string(),
        BomField::Field(name) => c
            .fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default(),
    })
}

/// Joins references sorted in natural order with `,`, writing runs of three or more consecutive
/// numbers with the same prefix as `first-last` if `ranges` (as KiCad does: `R1,R2,R4-R6`).
fn shorthand(references: &[String], ranges: bool) -> String {
    let split = |reference: &str| {
        let prefix = reference.trim_end_matches(|c: char| c.is_ascii_digit());
        let number = reference[prefix.len()..].parse::<u64>().ok()?;
        Some((prefix.to_string(), number))
    };
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < references.len() {
        let mut end = i + 1;
        if ranges && let Some((prefix, number)) = split(&references[i]) {
            while end < references.len()
                && split(&references[end])
                    .is_some_and(|(p, n)| p == prefix && n == number + (end - i) as u64)
            {
                end += 1;
            }
        }
        if end - i >= 3 {
            parts.push(format!("{}-{}", references[i], references[end - 1]));
        } else {
            parts.extend(references[i..end].iter().cloned());
        }
        i = end;
    }
    parts.join(",")
}

impl KicadBom {
    /// The columns.
    pub fn columns(&self) -> &[BomColumn] {
        &self.columns
    }

    /// The rows, sorted by their first reference in natural order.
    pub fn rows(&self) -> &[BomRow] {
        &self.rows
    }

    /// Writes the BOM to `writer` as CSV, as `kicad-cli sch export bom` does: a header with the
    /// labels, then a line per row, every cell quoted with `"` (doubled inside), tabs and line
    /// breaks removed.
    ///
    /// # Errors
    /// The error of `writer`.
    pub fn write(&self, writer: &mut impl io::Write) -> io::Result<()> {
        write!(writer, "{self}")
    }
}

impl fmt::Display for KicadBom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let line = |f: &mut fmt::Formatter<'_>, cells: &mut dyn Iterator<Item = &str>| {
            let cells: Vec<String> = cells
                .map(|cell| {
                    let cell: String = cell
                        .chars()
                        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
                        .collect();
                    format!("\"{}\"", cell.replace('"', "\"\""))
                })
                .collect();
            writeln!(f, "{}", cells.join(","))
        };
        line(f, &mut self.columns.iter().map(|c| c.label.as_str()))?;
        for row in &self.rows {
            line(f, &mut row.cells.iter().map(String::as_str))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        circuit::CircuitBuilder,
        kicad::PartFlags,
        test_parts::{Capacitor, CustomPart, Resistor},
    };

    /// `X` parts with the field `MPN`.
    fn custom(mpn: &str, flags: PartFlags) -> CustomPart {
        let mut part = CustomPart::valid();
        let info = part.kicad.as_mut().unwrap();
        info.fields = vec![("MPN".into(), mpn.into())];
        info.flags = flags;
        part
    }

    fn board() -> KicadNetlist {
        let mut circuit = CircuitBuilder::new();
        for _ in 0..5 {
            circuit.instantiate_part(Resistor("10k"));
        }
        circuit.instantiate_part(Resistor("1k"));
        circuit.instantiate_part(Capacitor("100n"));
        circuit.instantiate_part(Capacitor("100n"));
        let dnp = PartFlags {
            dnp: true,
            ..Default::default()
        };
        let excluded = PartFlags {
            exclude_from_bom: true,
            ..Default::default()
        };
        let off_board = PartFlags {
            exclude_from_board: true,
            ..Default::default()
        };
        circuit.instantiate_part(custom("A", PartFlags::default()));
        circuit.instantiate_part(custom("A", off_board));
        circuit.instantiate_part(custom("B", PartFlags::default()));
        circuit.instantiate_part(custom("A", dnp));
        circuit.instantiate_part(custom("A", excluded));
        circuit.build(&()).to_netlist().unwrap().to_kicad().unwrap()
    }

    #[test]
    fn test_grouped_csv() {
        let bom = board().bom(&BomOptions::default().with_field("MPN"));
        assert_eq!(
            bom.to_string(),
            r#""Refs","Value","Footprint","Qty","DNP","MPN"
"C1,C2","100n","Capacitor_SMD:C_0603_1608Metric","2","",""
"R1-R5","10k","Resistor_SMD:R_0603_1608Metric","5","",""
"R6","1k","Resistor_SMD:R_0603_1608Metric","1","",""
"X1,X2","custom","Custom:Footprint","2","","A"
"X3","custom","Custom:Footprint","1","","B"
"X4","custom","Custom:Footprint","1","DNP","A"
"#
        );
        let row = &bom.rows()[1];
        assert_eq!(row.references, ["R1", "R2", "R3", "R4", "R5"]);
        assert!(!row.dnp);
        assert!(bom.rows()[5].dnp);
    }

    /// DNP components are not grouped with populated ones even without a DNP column.
    #[test]
    fn test_dnp_is_a_separate_row() {
        let options = BomOptions {
            columns: vec![BomColumn::new(BomField::Reference, "Refs")],
            ..Default::default()
        };
        let refs: Vec<_> = board()
            .bom(&options)
            .rows()
            .iter()
            .map(|row| row.cells[0].clone())
            .collect();
        assert_eq!(refs, ["C1,C2,R1-R6,X1-X3", "X4"]);
    }

    #[test]
    fn test_ungrouped_without_dnp() {
        let options = BomOptions {
            columns: vec![
                BomColumn::new(BomField::Reference, "Reference"),
                BomColumn::new(BomField::LibSource, "Symbol"),
                BomColumn::new(BomField::Quantity, "Qty"),
            ],
            group: false,
            exclude_dnp: true,
            ..Default::default()
        };
        let bom = board().bom(&options);
        let lines: Vec<_> = bom.to_string().lines().map(str::to_string).collect();
        assert_eq!(lines.len(), 1 + 11);
        assert_eq!(lines[1], r#""C1","Device:C","1""#);
        assert_eq!(lines[11], r#""X3","Custom:Symbol","1""#);
    }

    #[test]
    fn test_shorthand() {
        let refs = |list: &[&str]| list.iter().map(|r| r.to_string()).collect::<Vec<_>>();
        let list = refs(&["R1", "R2", "R4", "R5", "R6", "R8", "U1", "U2", "U3", "X"]);
        assert_eq!(shorthand(&list, true), "R1,R2,R4-R6,R8,U1-U3,X");
        assert_eq!(shorthand(&list, false), "R1,R2,R4,R5,R6,R8,U1,U2,U3,X");
        // A run does not continue into another prefix.
        assert_eq!(shorthand(&refs(&["R1", "R2", "RN3"]), true), "R1,R2,RN3");
    }

    /// As kicad-cli: `"` is doubled, tabs and line breaks are removed.
    #[test]
    fn test_csv_escaping() {
        let bom = KicadBom {
            columns: vec![BomColumn::new(BomField::Value, "Va\"lue")],
            rows: vec![BomRow {
                references: vec!["C1".into()],
                dnp: false,
                cells: vec!["6\t8\r\n0n, \"x\"".into()],
            }],
        };
        assert_eq!(bom.to_string(), "\"Va\"\"lue\"\n\"680n, \"\"x\"\"\"\n");
    }
}
