# KiCad BOM (CSV) Format

`KicadNetlist::bom` (`src/kicad/bom.rs`) writes a bill of materials in the CSV format of
`kicad-cli sch export bom` with its default settings. KiCad has no specification of this output;
this document records what KiCad 10.0 was observed to write, and how to observe it again.

## 1. Reproducing the observations

Point the KiCad configuration directories into `$TMPDIR` where `$HOME` is read-only (e.g. in the
Claude Code sandbox), then export the BOM of a demo schematic:

```sh
export XDG_CONFIG_HOME=$TMPDIR/kicad/config XDG_CACHE_HOME=$TMPDIR/kicad/cache XDG_DATA_HOME=$TMPDIR/kicad/data
S=/usr/share/kicad/demos/openair-max/One-Air-Max.kicad_sch   # has DNP and grouped parts
kicad-cli sch export bom -o $TMPDIR/bom.csv $S
kicad-cli sch export bom --group-by 'Value,Footprint' -o $TMPDIR/grouped.csv $S
kicad-cli sch export bom --group-by Footprint --fields 'Reference,Value,Datasheet,QUANTITY' -o $TMPDIR/fp.csv $S
kicad-cli sch export bom --field-delimiter ';' --string-delimiter "'" --ref-delimiter ' ' \
  --ref-range-delimiter '' -o $TMPDIR/delims.csv $S
kicad-cli sch export bom --help        # the defaults of every option
```

`ecc83/ecc83-pp.kicad_sch` has components with `(in_bom no)`. To see the escaping, copy it to
`$TMPDIR` and put `"`, `,`, `\t` and `\n` into a `Value` property.

## 2. Observed behavior (KiCad 10.0)

- Default columns: the fields `Reference,Value,Footprint,QUANTITY,DNP`, labeled
  `Refs,Value,Footprint,Qty,DNP`. `QUANTITY` is the number of components in the row; `DNP` is
  `DNP` or empty.
- Every cell, including the header, is quoted with `"`; a `"` inside is doubled. Cells are
  separated by `,`, lines end with `\n`, and the last line too. Tabs and line breaks are removed
  from the cells (unless `--keep-tabs` / `--keep-line-breaks`).
- Components with `exclude_from_bom` (`(in_bom no)` in the schematic) are always left out
  (`--include-excluded-from-bom` is deprecated and has no effect). DNP components are listed unless
  `--exclude-dnp`. Components excluded from the board are listed.
- Without `--group-by` (the default), each component is a row. With it, components whose
  group-by fields are equal share a row. Rows are sorted by reference in natural order (`D10` after
  `D9`), a grouped row by its first reference.
- The references of a row are joined with `,`; three or more consecutive numbers with the same
  prefix are written as a range with `-` (`R8,R74-R78`, but `C6,C7`).
- With `--group-by Value,Footprint`, DNP and populated components with the same value and
  footprint share a row, and the row shows `DNP` (e.g. `C41,C71,C72` where only some are DNP).
- The group-by fields need not be output columns. A cell of a column the components of a row
  differ in lists their distinct values, without empty ones, sorted in string order and joined
  with `,` whatever `--ref-delimiter` is (with `--group-by Footprint`:
  `"0.1uF,10nF,1nF,1uF,..."`; with `--group-by Value`: `"easyeda2kicad:C0402,easyeda2kicad:C0603"`).
- `Datasheet` and `Description` columns show the symbol's fields; a `~` datasheet is an empty
  cell (as in the `.net`, which omits it: `doc/dev/kicad_netlist_format.md` section 3).
- `--field-delimiter` separates the cells, `--string-delimiter` surrounds them and is doubled
  inside (`'6''8"0,n'` with `'`); an empty string delimiter writes the cells as they are, with
  nothing escaped. `--ref-delimiter` separates the references and `--ref-range-delimiter` joins a
  range; an empty range delimiter lists every reference. The delimiters are taken literally
  (`'\t'` is the two characters; pass a real tab).

## 3. What cuprite_wire writes

- The format of section 2, with the default columns. `BomOptions` chooses the columns (also
  `LibSource`, `Datasheet`, `Description` and the part fields, e.g. `MPN`), the grouping, the four
  delimiters, and whether to leave out DNP components.
- Grouping is on by default (kicad-cli's is off) because a BOM for ordering is usually grouped.
  By default (`BomGrouping::Columns`) components are grouped when all their cells are equal
  except the references and the quantity, so a grouped row never hides a difference in a column
  that is written. `BomGrouping::Fields` is kicad-cli's `--group-by`, with the cells of section 2.
  Unlike KiCad, DNP and populated components are never in the same row, even without a DNP
  column: KiCad's mixed row would mark populated components as DNP.
- Only the information of the netlist is available (no `ki_*` fields or variants), and a
  value-less field is an empty cell.
