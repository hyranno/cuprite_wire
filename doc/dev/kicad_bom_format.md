# KiCad BOM (CSV) Format

`KicadNetlist::bom` (`src/kicad/bom.rs`) writes a bill of materials in the CSV format of
`kicad-cli sch export bom` with its default settings. KiCad has no specification of this output;
this document records what KiCad 10.0 was observed to write, and how to observe it again.

## 1. Reproducing the observations

Set the KiCad configuration directories as in the `kicad` skill (`$HOME` is read-only in the
sandbox), then export the BOM of a demo schematic:

```sh
export XDG_CONFIG_HOME=$TMPDIR/kicad/config XDG_CACHE_HOME=$TMPDIR/kicad/cache XDG_DATA_HOME=$TMPDIR/kicad/data
S=/usr/share/kicad/demos/openair-max/One-Air-Max.kicad_sch   # has DNP and grouped parts
kicad-cli sch export bom -o $TMPDIR/bom.csv $S
kicad-cli sch export bom --group-by 'Value,Footprint' -o $TMPDIR/grouped.csv $S
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

## 3. What cuprite_wire writes

- The format of section 2, with the default columns. `BomOptions` chooses the columns (also
  `LibSource` and the part fields, e.g. `MPN`), whether to group and to write ranges, and whether to
  leave out DNP components.
- Grouping is on by default (kicad-cli's is off) because a BOM for ordering is usually grouped.
  Components are grouped when all their cells are equal except the references and the quantity,
  so a grouped row never hides a difference in a column that is written. Unlike KiCad, DNP and
  populated components are never in the same row, even without a DNP column: KiCad's mixed row
  would mark populated components as DNP.
- Only the information of the netlist is available: there are no `Datasheet` / `Description`
  columns, and a value-less field is an empty cell.
