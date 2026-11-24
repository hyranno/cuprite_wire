//! Bill of materials (BOM) of a KiCad netlist, written as CSV like `kicad-cli sch export bom`.
//!
//! See the guide [Exporting a KiCad netlist](https://github.com/hyranno/cuprite_wire/blob/main/doc/kicad_netlist_usage.md).

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, io,
};

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
    /// The datasheet ([`KicadPartInfo::datasheet`](crate::kicad::KicadPartInfo::datasheet),
    /// empty if not set).
    Datasheet,
    /// The description
    /// ([`KicadPartInfo::description`](crate::kicad::KicadPartInfo::description), empty if not
    /// set).
    Description,
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

/// Which components share a row of a BOM. Populated and not populated (DNP) components are
/// never in the same row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BomGrouping {
    /// A row per component (kicad-cli's default).
    None,
    /// Components whose cells are all equal, except the references and the quantity, so a row
    /// never hides a difference in a column.
    #[default]
    Columns,
    /// Components whose given fields are equal, written or not (kicad-cli's `--group-by`). A cell
    /// of a row whose components differ lists their values, without empty ones, sorted and
    /// joined with `,` (`1nF,22uF`), as kicad-cli writes it. [`BomField::Reference`] and
    /// [`BomField::Quantity`] are ignored.
    Fields(Vec<BomField>),
}

/// How to make a BOM. The default is the default of `kicad-cli sch export bom`, with the components
/// grouped: the columns `Refs`, `Value`, `Footprint`, `Qty` and `DNP`, reference ranges, and
/// cells quoted with `"` and separated by `,`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BomOptions {
    /// The columns, in order.
    pub columns: Vec<BomColumn>,
    /// Which components share a row.
    pub group: BomGrouping,
    /// Leaves out the components that are not populated (DNP).
    pub exclude_dnp: bool,
    /// The separator of the cells (kicad-cli's `--field-delimiter`).
    pub field_delimiter: String,
    /// What surrounds every cell, doubled inside a cell (kicad-cli's `--string-delimiter`). Empty
    /// for none: cells are then written as they are.
    pub string_delimiter: String,
    /// The separator of the references of a row (kicad-cli's `--ref-delimiter`).
    pub ref_delimiter: String,
    /// The separator of three or more consecutive references written as a range (`R3-R5`;
    /// kicad-cli's `--ref-range-delimiter`). Empty to list them all.
    pub ref_range_delimiter: String,
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
            group: BomGrouping::Columns,
            exclude_dnp: false,
            field_delimiter: ",".into(),
            string_delimiter: "\"".into(),
            ref_delimiter: ",".into(),
            ref_range_delimiter: "-".into(),
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
    field_delimiter: String,
    string_delimiter: String,
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
        let key_fields: Vec<&BomField> = match &options.group {
            BomGrouping::None => Vec::new(),
            BomGrouping::Columns => options.columns.iter().map(|c| &c.field).collect(),
            BomGrouping::Fields(fields) => fields.iter().collect(),
        };
        // The cells of the fields of the key, `None` for the references and the quantity.
        let key = |c: &KicadComponent| -> (bool, Vec<Option<String>>) {
            let cells = key_fields.iter().map(|f| component_cell(c, f)).collect();
            (c.flags.dnp, cells)
        };
        let mut groups: Vec<Vec<&KicadComponent>> = if options.group == BomGrouping::None {
            components.map(|c| vec![c]).collect()
        } else {
            let mut groups: BTreeMap<_, Vec<_>> = BTreeMap::new();
            for c in components {
                groups.entry(key(c)).or_default().push(c);
            }
            groups.into_values().collect()
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
                        BomField::Reference => shorthand(&references, options),
                        BomField::Quantity => references.len().to_string(),
                        field => {
                            // Equal in every component unless grouped by other fields.
                            let values: BTreeSet<String> = group
                                .iter()
                                .filter_map(|c| component_cell(c, field))
                                .filter(|value| !value.is_empty())
                                .collect();
                            values.into_iter().collect::<Vec<_>>().join(",")
                        }
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
            field_delimiter: options.field_delimiter.clone(),
            string_delimiter: options.string_delimiter.clone(),
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
        BomField::Datasheet => c.datasheet.clone(),
        BomField::Description => c.description.clone(),
        BomField::Dnp => if c.flags.dnp { "DNP" } else { "" }.to_string(),
        BomField::Field(name) => c
            .fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default(),
    })
}

/// Joins references sorted in natural order with the `ref_delimiter` of `options`, writing runs of
/// three or more consecutive numbers with the same prefix as `first-last` with its
/// `ref_range_delimiter` unless that is empty (as KiCad does: `R1,R2,R4-R6`).
fn shorthand(references: &[String], options: &BomOptions) -> String {
    let ranges = !options.ref_range_delimiter.is_empty();
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
            parts.push(format!(
                "{}{}{}",
                references[i],
                options.ref_range_delimiter,
                references[end - 1]
            ));
        } else {
            parts.extend(references[i..end].iter().cloned());
        }
        i = end;
    }
    parts.join(&options.ref_delimiter)
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
    /// labels, then a line per row, every cell surrounded by the `string_delimiter` of the
    /// options (doubled inside) and separated by their `field_delimiter`, tabs and line breaks
    /// removed.
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
                    let quote = &self.string_delimiter;
                    if quote.is_empty() {
                        cell
                    } else {
                        format!("{quote}{}{quote}", cell.replace(quote, &quote.repeat(2)))
                    }
                })
                .collect();
            writeln!(f, "{}", cells.join(&self.field_delimiter))
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
            group: BomGrouping::None,
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
        let options = BomOptions::default();
        assert_eq!(shorthand(&list, &options), "R1,R2,R4-R6,R8,U1-U3,X");
        let no_ranges = BomOptions {
            ref_delimiter: " ".into(),
            ref_range_delimiter: String::new(),
            ..Default::default()
        };
        assert_eq!(shorthand(&list, &no_ranges), "R1 R2 R4 R5 R6 R8 U1 U2 U3 X");
        let ranges = BomOptions {
            ref_range_delimiter: "..".into(),
            ..Default::default()
        };
        assert_eq!(shorthand(&list, &ranges), "R1,R2,R4..R6,R8,U1..U3,X");
        // A run does not continue into another prefix.
        assert_eq!(
            shorthand(&refs(&["R1", "R2", "RN3"]), &options),
            "R1,R2,RN3"
        );
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
            field_delimiter: ",".into(),
            string_delimiter: "\"".into(),
        };
        assert_eq!(bom.to_string(), "\"Va\"\"lue\"\n\"680n, \"\"x\"\"\"\n");
    }

    /// As kicad-cli: the string delimiter is doubled inside a cell; without one, cells are written
    /// as they are.
    #[test]
    fn test_delimiters() {
        let options = |string_delimiter: &str| BomOptions {
            columns: vec![
                BomColumn::new(BomField::Reference, "Refs"),
                BomColumn::new(BomField::Field("MPN".into()), "M'PN"),
            ],
            field_delimiter: ";".into(),
            string_delimiter: string_delimiter.into(),
            ..Default::default()
        };
        let lines = |string_delimiter| {
            let bom = board().bom(&options(string_delimiter)).to_string();
            bom.lines().take(2).map(str::to_string).collect::<Vec<_>>()
        };
        assert_eq!(lines("'"), ["'Refs';'M''PN'", "'C1,C2,R1-R6';''"]);
        assert_eq!(lines(""), ["Refs;M'PN", "C1,C2,R1-R6;"]);
    }

    #[test]
    fn test_datasheet_and_description() {
        let mut circuit = CircuitBuilder::new();
        let mut part = CustomPart::valid();
        let info = part.kicad.as_mut().unwrap();
        info.datasheet = Some("https://example.com/x.pdf".into());
        info.description = Some("A part".into());
        circuit.instantiate_part(part);
        let mut tilde = CustomPart::valid();
        tilde.kicad.as_mut().unwrap().datasheet = Some("~".into());
        circuit.instantiate_part(tilde);
        let netlist = circuit.build(&()).to_netlist().unwrap().to_kicad().unwrap();
        let options = BomOptions {
            columns: vec![
                BomColumn::new(BomField::Reference, "Refs"),
                BomColumn::new(BomField::Datasheet, "Datasheet"),
                BomColumn::new(BomField::Description, "Description"),
            ],
            ..Default::default()
        };
        assert_eq!(
            netlist.bom(&options).to_string(),
            r#""Refs","Datasheet","Description"
"X1","https://example.com/x.pdf","A part"
"X2","",""
"#
        );
    }

    /// As kicad-cli's `--group-by`: the other cells list the distinct values that are not empty.
    #[test]
    fn test_group_by_fields() {
        let options = BomOptions {
            columns: vec![
                BomColumn::new(BomField::Reference, "Refs"),
                BomColumn::new(BomField::Value, "Value"),
                BomColumn::new(BomField::Field("MPN".into()), "MPN"),
                BomColumn::new(BomField::Quantity, "Qty"),
            ],
            group: BomGrouping::Fields(vec![BomField::Footprint]),
            ..Default::default()
        };
        assert_eq!(
            board().bom(&options).to_string(),
            r#""Refs","Value","MPN","Qty"
"C1,C2","100n","","2"
"R1-R6","10k,1k","","6"
"X1-X3","custom","A,B","3"
"X4","custom","A","1"
"#
        );
    }
}
