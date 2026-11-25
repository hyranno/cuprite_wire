# KiCad Netlist (.net) Format

This document describes the netlist that cuprite_wire emits for import into the PCB editor (pcbnew) of KiCad 9 and 10.
The format is KiCad's S-expression netlist (the "KiCad" format of Eeschema, `version "E"`).

> KiCad publishes no official specification of this format
> (the File Formats section of dev-docs has no netlist page).
> This document was written by the project authors in their own words, based on:
>
> 1. Actual output of `kicad-cli sch export netlist --format kicadsexpr` from KiCad 9.0.2
> 2. The behavior of the KiCad 9.0 source code (the Eeschema exporter and the pcbnew reader)
> 3. For KiCad 10 (sections 6.1.1, 6.2 and 6.3), its output and source code
>
> See "References" at the end for details.

## 1. Overall structure

```
(export (version "E")
  (design ...)        ; skipped by pcbnew
  (components
    (comp ...) ...)   ; components
  (libparts ...)      ; library part definitions; pcbnew only uses footprint filters and pin count
  (libraries ...)     ; skipped by pcbnew
  (nets
    (net ...) ...))   ; nets
```

- The top level is a single `(export ...)`.
- Eeschema writes `"E"` as `version`. pcbnew reads the value but does not validate it. We emit `"E"` for compatibility.
- pcbnew requires only `components` and `nets`. `design` and `libraries` are skipped,
  and `libparts` may be omitted (see section 4).

### Lexical rules

- Tokens (keywords) consist of lowercase letters and `_` only.
- Strings are enclosed in `"` and encoded in UTF-8. A `"` inside a string is escaped as `\"`
  (e.g. `(value "c=\"\"")`).
- Control characters in strings: KiCad 10's lexer keeps them as they are, except that a raw
  NUL ends the string (the rest is lost) and an escaped one (`\000`) is not a character either.
  Checked with `pcbnew.FootprintLoad` on a footprint whose `descr` holds raw `\x01 \x7f \x1b`
  (kept), a raw NUL (`"a\0b"` read as `a`) and `\001` (read as `\x01`); the netlist reader uses
  the same lexer. So cuprite_wire writes control characters other than `\n \r \t` raw, rejects
  them in names and labels (`InvalidName`), and reports a NUL in any other text of a part
  (`KicadNulCharacter`).
- KiCad 9 quotes numeric-looking values (`code`, `pin`, etc.) as strings too.
  The reader accepts both quoted and unquoted forms, but we always quote to match KiCad.
- Indentation and line breaks are insignificant.

## 2. `design` section (optional)

pcbnew does not use its contents. It may be emitted as human-readable information.

```
(design
  (source "my_board.rs")        ; source file
  (date "2026-11-15T11:00:32+0000")
  (tool "cuprite_wire 0.1.0")
  (sheet (number "1") (name "/") (tstamps "/")
    (title_block (title) (company) (rev) (date) (source "my_board.rs"))))
```

Eeschema writes one `sheet` per hierarchical sheet, and `title_block` contains `comment` 1 through 9.
Since this project has no schematic, `design` can be omitted or kept minimal.

## 3. `components` section

Write one `comp` per component (i.e. per reference designator).

```
(comp (ref "R1")
  (value "10k")
  (footprint "Resistor_SMD:R_0603_1608Metric")
  (datasheet "https://example.com/r.pdf")          ; optional
  (description "Resistor")                          ; optional
  (fields
    (field (name "Footprint") "Resistor_SMD:R_0603_1608Metric")
    (field (name "Datasheet"))
    (field (name "Description")))
  (libsource (lib "Device") (part "R") (description "Resistor"))
  (property (name "Sheetname") (value ""))
  (property (name "Sheetfile") (value ""))
  (property (name "ki_fp_filters") (value "R_*"))
  (sheetpath (names "/") (tstamps "/"))
  (tstamps "0f3c1a2e-5b7d-4c11-9a0e-3d2b6f8e9a01"))
```

| Element | Required | Handling in pcbnew / notes |
|---|---|---|
| `ref` | required | Reference designator (e.g. `R1`). Referenced by `node` in nets. Must be unique on the board. |
| `value` | recommended | Becomes the footprint's Value. Eeschema writes `"~"` when empty. |
| `footprint` | effectively required | `Library:Footprint` (LIB_ID). Must exist in pcbnew's footprint library table to be placed. If empty, the component is reported as having no footprint assigned. A malformed LIB_ID is a read error. |
| `datasheet`, `description` | optional | The symbol's `Datasheet` / `Description` fields. Eeschema omits them when empty, and a `~` datasheet too (KiCad 10.0: `F2` of `demos/openair-max/Battery-Holder.kicad_sch`, a `Device:Fuse` with `~`). |
| `fields` / `field` | optional | `(field (name "NAME") "VALUE")`; the value is omitted when empty. pcbnew imports them as footprint fields. |
| `libsource` | optional | Only `lib` / `part` / `description` children are allowed (any other child is a read error). Serves as the key to match `libparts`. |
| `property` | optional | `(property (name "NAME") (value "VALUE"))`. Some are value-less flags (see below). |
| `sheetpath` | recommended | `names` is the human-readable hierarchical path; `tstamps` is the hierarchical path of UUIDs (separated by `/`, with a trailing `/`). Both are `"/"` at the root. |
| `component_classes` | optional | `(component_classes (class (name "X")) ...)`. For the KiCad 9 component class feature. |
| `tstamps` | recommended | Symbol UUID(s). Multiple values may be listed (see 3.2). |

Any other unknown element is skipped by pcbnew.

### 3.1 Flag-type and special properties

| name | Meaning |
|---|---|
| `exclude_from_board` | No value: `(property (name "exclude_from_board"))`. Not placed on the board. |
| `exclude_from_bom` | No value. Excluded from the BOM. |
| `dnp` | No value. Do Not Populate. |
| `ki_fp_filters` | Footprint filters (space-separated globs). |
| `ki_keywords` | Symbol keywords. |
| `Sheetname`, `Sheetfile` | Sheet name and sheet file name added by Eeschema. |

### 3.2 `tstamps` and component matching in pcbnew

When updating from a netlist, pcbnew matches existing footprints to netlist components by
**the `sheetpath` `tstamps` + the `comp` `tstamps` (i.e. the symbol path)**.
If "Link footprints using symbol references" is selected in the dialog, matching uses the reference instead.

- Unless the **same UUID** is emitted for the same component on every export, pcbnew treats the
  footprint as a different one and it must be placed again. cuprite_wire should derive UUIDs
  deterministically from component identity (e.g. UUID v5 with a fixed namespace).
- For multi-unit parts (e.g. a dual op-amp), Eeschema has one symbol per unit, so they are merged into
  one `comp` whose `tstamps` lists several UUIDs
  (`(tstamps "uuid-A" "uuid-B" "uuid-C")`). pcbnew keeps all of them.
  Eeschema picks the unit with the smallest UUID as the primary one and writes it last, the other
  units first in the order of the sheet's file. pcbnew (KiCad 10) matches a footprint against any
  of the UUIDs, but sets a new footprint's path from the **first** one (and reports a path change
  if a matched footprint has another). cuprite_wire writes one UUID per unit, the component's own
  UUID first (unchanged from a single-unit part, so existing boards keep their paths) and UUIDs
  derived from it for units 2, 3, ... (ref. 7, 8).
- UUIDs are lowercase hex in the hyphenated `8-4-4-4-12` form.
- The `tstamps` inside `sheetpath` is read as a single path string.

## 4. `libparts` section (optional)

```
(libparts
  (libpart (lib "Device") (part "R")
    (description "Resistor")
    (docs "~")
    (footprints
      (fp "R_*"))
    (fields
      (field (name "Reference") "R")
      (field (name "Value") "R"))
    (pins
      (pin (num "1") (name "") (type "passive"))
      (pin (num "2") (name "") (type "passive")))))
```

- For components whose `libsource` (`lib`, `part`) matches, pcbnew only sets
  the footprint filters from `footprints` and the pin count from the number of `pins`.
  These mainly serve footprint-assignment tools, so this project, which always specifies `footprint`, may omit the section.
- `aliases` / `alias` are also read, but current Eeschema does not emit them.
- `type` of `pin` is the electrical type of the symbol pin (same vocabulary as `pintype` in section 6).

## 5. `libraries` section (optional)

```
(libraries
  (library (logical "Device")
    (uri "/usr/share/kicad/symbols/Device.kicad_sym")))
```

Skipped by pcbnew. Not needed.

## 6. `nets` section

```
(nets
  (net (code "1") (name "GND") (class "Default")
    (node (ref "C1") (pin "2") (pinfunction "2") (pintype "passive"))
    (node (ref "U1") (pin "4") (pinfunction "V-") (pintype "power_in")))
  (net (code "2") (name "/VOUT") (class "Default")
    (node (ref "R1") (pin "2") (pintype "passive"))
    (node (ref "U1") (pin "1") (pinfunction "OUT") (pintype "output"))))
```

| Element | Required | Handling in pcbnew / notes |
|---|---|---|
| `code` | required | Integer >= 1. Nets with code <= 0 are ignored. Must be unique within the file. |
| `name` | recommended | Net name. If empty, pcbnew assigns a placeholder `N-00000<code>`. |
| `class` | optional | Net class name. **Skipped by the pcbnew 9 netlist reader** (net classes are determined by pattern assignments in the project settings `.kicad_pro`). We emit `"Default"` for compatibility. |
| `node` | one or more | A connected pin. |

Children of `node`:

| Element | Required | Notes |
|---|---|---|
| `ref` | required | `ref` of a `comp`. Nodes of components that do not exist (or are excluded by `exclude_from_board` etc.) are ignored. |
| `pin` | required | Must match a **footprint pad number** (not a symbol pin name). |
| `pinfunction` | optional | Pin name. Eeschema omits it when the pin name is empty. pcbnew shows it as the pad's "pin function". |
| `pintype` | optional | Electrical type of the pin: `input` `output` `bidirectional` `tri_state` `passive` `free` `unspecified` `power_in` `power_out` `open_collector` `open_emitter` `no_connect`. A pin alone on a net with a no-connect flag gets a `+no_connect` suffix (e.g. `passive+no_connect`); see 6.2. |

### 6.1 Net naming conventions (following Eeschema)

Net names are arbitrary strings to pcbnew, but using Eeschema's naming fits better with KiCad features
(such as net class pattern assignments).

| Kind | Example | Rule |
|---|---|---|
| Global (power symbol, global label) | `GND`, `+3V3` | The name itself |
| Local / hierarchical label | `/VOUT`, `/amp_left/IN` | Sheet path + label name |
| Unnamed | `Net-(R1-Pad2)`, `Net-(U1-+)` | `Net-(<ref>-<pin name or Pad<number>>)` |
| Single unconnected pin | `unconnected-(U1-NC-Pad7)` | A net with only one pin |

- For an unnamed net, Eeschema prefers a pin that is written by its name alone (`U1-OUT`) to one
  written with `Pad<number>` (a pin without a name, named as its pad number, or whose name is
  shared by several pins of the symbol or unit, 6.1.1), and among those takes the smallest net name in string
  order. The pin type does not matter. (Observed with KiCad 9.0.2 using
  `tools/compare_kicad_cli_netlist.py`.)
- Characters such as `/` in net names are escaped by Eeschema (e.g. `/` -> `{slash}`).
  When this project uses arbitrary strings as names, follow the same `{slash}`-style escaping.
- Eeschema sorts nets by name in natural order (case-sensitive: `/B` < `/_x` < `/a9` < `/a10` < `/b`)
  and assigns `code` from 1.
  Nets containing only power-symbol (`#PWR`) pins are not emitted, so gaps in `code` may appear.
  pcbnew does not care about such gaps.
- Nodes are sorted by `ref` (string order) then by pin number, with duplicates removed.
- Components are written sheet by sheet in page order, and by reference in natural order in each
  sheet (a root `X1` comes before `C1` on page 2).

### 6.1.1 Multi-unit symbols

Observed with KiCad 10.0 on the `ecc83` demo (a three-unit valve) and on a copy with a
`unit_name` added, and explained by `SCH_PIN::GetDefaultNetName` and `SCH_SYMBOL::GetRef` (ref. 9):

- A pin written by its name uses the reference **with the unit's suffix**: `Net-(U1A-K)`,
  `Net-(U1B-K)`. The suffix is the unit's `unit_name` if the library symbol has one
  (`Net-(U1Tri-K)`), otherwise letters `A`..`Z`, `AA`, `AB`, ... (`LIB_SYMBOL::LetterSubReference`,
  with the default schematic settings: no separator, first letter `A`). Symbols with one unit get
  no suffix. A pin written as `Pad<number>` never gets the suffix (`Net-(U1-Pad3)`), since pad
  numbers are unique in the footprint.
- "Shared name" (which adds `-Pad<number>`) is decided among the pins of the **placed unit**
  (its own pins and those of unit 0): another pin with the same shown name, a different shown
  number, and the same no-connect-ness as the net (`unconnected` iff the other pin is of type
  `no_connect`). So the K of each unit of `Diode:BAT54SDW` is not shared, and the net of pad 6 is
  `Net-(D1A-K)`.
- Each placed unit has its own copies of the unit-0 pins (and of a pin repeated in several
  units), so each copy is a candidate and the smallest name wins as usual: `U1A-...` for letters.
- cuprite_wire keeps one pin per pad with the **lowest** unit of a repeated pin
  (`KicadPinInfo::unit`), and treats a unit-0 pin as a candidate in every unit. A pin repeated in
  units 1 and 2 is therefore only judged in unit 1; KiCad would also consider unit 2, which only
  matters if its name is shared in unit 1 but not in unit 2 (none of the 17 symbols of the KiCad
  10 libraries with pins repeated in some units have that). A stacked pin in KiCad 10's notation
  (`[2,13]`) is one library pin, so its pads do not share a name in KiCad, but they are separate
  pins here and do (the name gets `-Pad`).
- KiCad 10's own format adds `(units (unit (name "A") (pins (pin (num "1")) ...)) ...)` to each
  component; cuprite_wire writes the 9 format without it.

### 6.2 The `+no_connect` suffix

Observed with KiCad 10.0 (`kicad-cli sch export netlist`) on the demos `ecc83/ecc83-pp`,
`complex_hierarchy` and `cm5_minima`, and on schematics synthesized by
`tools/compare_kicad_cli_netlist.py --keep DIR` with no-connect flags added by hand:

| Net | `pintype` |
|---|---|
| one pin, with a no-connect flag (with or without a label) | `<type>+no_connect` |
| one pin, without a flag (also a `no_connect` pin) | `<type>` |
| several pins, with a flag (ERC `no_connect_connected`) | `<type>` for every pin |

A flagged net of several pins without a label is named `unconnected-(...)` instead of `Net-(...)`
(e.g. `unconnected-(R1-Pad2)` for R1 pad 2 and U1 pin `-`, whose name would otherwise be
`Net-(U1--)`), whichever pin the flag is on. cuprite_wire keeps the `Net-(...)` name: the net is
an ERC error anyway, and the rule picking the pin was not worked out.

`tools/compare_kicad_cli_netlist.py` places a no-connect flag on every pin whose type has the
suffix, so the golden `multi_pin_ic.net` checks the suffix against kicad-cli.

### 6.3 Hidden power input pins

Observed with KiCad 10.0 (`kicad-cli sch export netlist` and `kicad-cli sch erc`) on hand-written
schematics: a symbol with input/output pins and hidden `power_in` pins `VCC` and `GND`
(`(pin power_in line ... (hide yes) (name "VCC") ...)`), placed several times, with resistors
wired to labels.

- A hidden `power_in` pin is connected to the global net named after the pin (`VCC`, `GND`), with
  no wire or label: the pins of every instance and a global label `VCC` are on one net `VCC`.
- What is wired to the pin joins that net too: a pin wired to a local label `X` puts `X`'s net on
  `VCC`, named `VCC` (ERC `multiple_net_names`).
- ERC treats the pins as other `power_in` pins: `power_pin_not_driven` once per net, and
  `pin_not_connected` for a pin alone on its net, shown as "Hidden pin 2 [VCC, Power input, Line]".
- A local label of the root sheet with the same name (`VCC`) is on the same net, with ERC
  `same_local_global_label`. This is not specific to hidden pins: a root local label `A` and a
  global label `A` are one net `A` as well. cuprite_wire keeps local and global labels apart
  (`/A` and `A` are different nets).

The KiCad 10 libraries have no hidden `power_in` pins outside power symbols (counted over all
`symbols/*.kicad_sym`, `extends` resolved; KLC forbids them), and the pins of their power symbols
(`(power)`) are not hidden. Older and third-party libraries still have them. `kicad_part!` gives
such a part `Part::global_labels`, a global label on each of these pins.

## 7. Minimal form

The minimal netlist that pcbnew can read looks like this
(omitting `libsource` / `sheetpath` / `tstamps` / `libparts` / `libraries` / `design`).
However, without `tstamps` components cannot be matched when updating from a netlist, so this project emits them.

```
(export (version "E")
  (components
    (comp (ref "R1") (value "10k") (footprint "Resistor_SMD:R_0603_1608Metric"))
    (comp (ref "C1") (value "100n") (footprint "Capacitor_SMD:C_0603_1608Metric")))
  (nets
    (net (code "1") (name "/OUT")
      (node (ref "R1") (pin "2"))
      (node (ref "C1") (pin "1")))
    (net (code "2") (name "GND")
      (node (ref "C1") (pin "2")))
    (net (code "3") (name "/IN")
      (node (ref "R1") (pin "1")))))
```

## 8. What cuprite_wire writes

See `doc/kicad_netlist_usage.md` section 4 for which elements cuprite_wire writes and how it orders
them, and `doc/dev/verifying.md` for how the output is checked against pcbnew and Eeschema.

## References

| # | Source | Used for | License |
|---|---|---|---|
| 1 | Actual output of KiCad 9.0.2 `kicad-cli sch export netlist --format kicadsexpr` (run on the bundled demos `demos/simulation/sallen_key`, `demos/complex_hierarchy`, `demos/ecc83`, `demos/stickhub`) | Element structure, output order, value formats | The output itself is not reproduced here. All examples in this document were written for this project. |
| 2 | KiCad source `pcbnew/netlist_reader/kicad_netlist_parser.cpp` (9.0 branch) <https://gitlab.com/kicad/code/kicad/-/blob/9.0/pcbnew/netlist_reader/kicad_netlist_parser.cpp> | Elements pcbnew reads/skips, error conditions | GPL-2.0-or-later |
| 3 | KiCad source `eeschema/netlist_exporters/netlist_exporter_xml.cpp` (9.0 branch) <https://gitlab.com/kicad/code/kicad/-/blob/9.0/eeschema/netlist_exporters/netlist_exporter_xml.cpp> | Output conditions, multi-unit `tstamps`, net/node ordering, `+no_connect` | GPL-2.0-or-later |
| 4 | KiCad source `common/netlist.keywords` (9.0 branch) <https://gitlab.com/kicad/code/kicad/-/blob/9.0/common/netlist.keywords> | List of keywords used | GPL-2.0-or-later |
| 5 | KiCad Developer Documentation, "S-Expression Format" <https://dev-docs.kicad.org/en/file-formats/sexpr-intro/> | General S-expression lexical rules | GPL-3.0 (LICENSE of the kicad/services/kicad-dev-docs repository) |
| 6 | "KiCad without Eeschema or CvPcb" <https://www.sheep-thrills.net/KiCad_without_Eeschema_or_CvPcb.html> | Hints for the minimal form (note: targets KiCad 2013, the older unquoted `version D` format) | None stated (treated as copyrighted by the author; content is not reproduced) |
| 7 | KiCad source `pcbnew/netlist_reader/kicad_netlist_reader.cpp` and `board_netlist_updater.cpp` (10.0 branch) <https://gitlab.com/kicad/code/kicad/-/tree/10.0/pcbnew/netlist_reader> | How pcbnew uses the `tstamps` of a component (path from the first, matching by any) | GPL-3.0-or-later; read for reference only |
| 8 | KiCad source `eeschema/netlist_exporters/netlist_exporter_xml.cpp` (10.0 branch) | Order of the multi-unit `tstamps`, the `units` element of the KiCad 10 format | GPL-3.0-or-later; read for reference only |
| 9 | KiCad source `eeschema/sch_pin.cpp`, `sch_symbol.cpp`, `lib_symbol.cpp` and `sch_io/kicad_sexpr/sch_io_kicad_sexpr_parser.cpp` (10.0 branch) | Automatic net names of multi-unit symbols, unit suffixes (`unit_name`, letters) | GPL-3.0-or-later; read for reference only |

### License notes

- This document does not reproduce code or text from the sources above; it describes facts
  (the structure and behavior of the format) independently. Since the structure of a file format itself
  is not a copyrightable work, we take the position that neither this document nor this project's
  output implementation is subject to the GPL of those sources.
- **Do not copy or port KiCad source code** into the implementation (the GPL would propagate).
  Reading it to understand behavior is fine, but write the code independently.
- Do not commit reference outputs generated from the KiCad demos, as they contain demo data
  (part names, net names, UUIDs, etc.). Generate expected-output files for tests from circuits created in this project.
- Reference 6 has no license statement, so do not quote or reproduce it.
