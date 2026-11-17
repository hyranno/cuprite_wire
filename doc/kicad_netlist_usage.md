# Exporting a KiCad Netlist

This document explains how to describe a circuit with cuprite_wire and export it as a KiCad 9
netlist (`.net`) for pcbnew. See `doc/kicad_netlist_format.md` for the file format itself.
A complete example is in `examples/kicad_rc_filter.rs`
(`cargo run --example kicad_rc_filter > rc_filter.net`).

## 1. Pipeline

```rust
let netlist = circuit             // CircuitBuilder
    .build(&options)              // BuiltCircuit: subcircuits expanded (never fails)
    .to_netlist()?                // Netlist: format-independent, nets resolved and named
    .to_kicad()?                  // KicadNetlist: KiCad information resolved and checked
    .with_source("my_board.rs");  // optional `source` in the `design` section
netlist.write(&mut file)?;        // or `netlist.to_string()`
```

- `build()` always succeeds. Problems found while describing the circuit (invalid names, ...) are
  kept and reported by `to_netlist()`.
- `to_netlist()` and `to_kicad()` return `Err(CircuitErrors)` with **all** errors found, not just the first.
- The output is deterministic: the same circuit always gives the same bytes.

## 2. Defining parts

`Part` is format-independent. Format-specific information is returned as an `Option`:

```rust
impl Part for Passive {
    fn get_ref_base_name(&self) -> String { "R".into() }      // reference prefix
    fn pin_ids(&self) -> Vec<String> { vec!["1".into(), "2".into()] }
    fn value(&self) -> String { "10k".into() }
    fn kicad(&self) -> Option<KicadPartInfo> {
        Some(KicadPartInfo {
            footprint: Some("Resistor_SMD:R_0603_1608Metric".parse().unwrap()),
            pins: BTreeMap::from([
                ("1".into(), KicadPinInfo::new("1", None, PinType::Passive)),
                ("2".into(), KicadPinInfo::new("2", None, PinType::Passive)),
            ]),
            lib_source: Some("Device:R".parse().unwrap()),
            ..Default::default()   // fields, flags (dnp, exclude_from_bom, exclude_from_board)
        })
    }
    // fn spice(&self) -> Option<SpicePartInfo>   (None by default)
    // fn internal_connections(&self) -> Vec<Vec<String>>   (empty by default)
}
```

- **One pin ID corresponds to one footprint pad**, as in KiCad symbols. `KicadPinInfo` gives the pad
  number (`pin` in the netlist), the pin name (`pinfunction`) and the electrical type (`pintype`).
  `KicadPartInfo::pins` needs exactly one entry per pin ID, and pads must be unique within a part.
- A part with several pads of the same function (e.g. six GND pins) has one pin ID per pad.
  Pins connected inside the part are declared with `internal_connections()`; they are connected
  when the part is instantiated (like stacked pins in a KiCad symbol).
- `LibId` (`Library:Name`) is checked when it is parsed or created with `LibId::new`.
- Whether a pad number exists in the footprint is not checked (it would require reading the
  footprint libraries). Use `tools/check_netlist_footprints.py` (section 6).

### Parts that do not support a format

Parts supporting different formats may be mixed in one circuit (e.g. a simulation-only voltage
source next to a connector that has no SPICE model). Only exporting such a circuit to the
unsupported format is an error: a part whose `kicad()` returns `None` makes `to_kicad()` fail with
`KicadUnsupported`. There is currently no option to skip unsupported parts.

## 3. Describing circuits

```rust
let mut circuit = CircuitBuilder::new();
let node = circuit.add_node();
let r = circuit.instantiate_part(Box::new(resistor));          // automatic name
let c = circuit.instantiate_part_named("c_bypass", Box::new(capacitor));
circuit.connect(node, r["1"]);
circuit.label(c["2"], NetLabel::global("GND"));
let pins = circuit.instantiate_subcircuit_named("amp_left", generator, &interface_nodes, |o| o.clone());
```

All of these record the calling source location with `#[track_caller]`, which is used in error
messages (section 5).

### Instance names and UUIDs

pcbnew matches footprints on the board to netlist components by UUID (`tstamps`), so the UUIDs
must not change between exports. cuprite_wire derives them (UUID v5) from the **hierarchical path
and the instance name** of each part, never from reference designators or source locations.

- `instantiate_part_named` / `instantiate_subcircuit_named` give an explicit name. Names must be
  non-empty, must not contain `#`, and must be unique per kind in one (sub)circuit.
- `instantiate_part` / `instantiate_subcircuit` give an automatic name `<base>#<n>`, numbered per
  kind in each (sub)circuit: the reference prefix for parts (`R#1`, `R#2`) and
  `SubcircuitGenerator::base_name()` for subcircuits (by default the type name without module path
  and generic arguments, e.g. `RcStage#1`).

> **Note:** an automatic name changes when a part of the same kind is added or removed before it
> in the same (sub)circuit, and changing an automatic name to an explicit one changes it once.
> Either changes the UUID, so pcbnew no longer links the footprint to the component.
> Give explicit names to parts whose placement you want to keep.

Reference designators (`R1`, `R2`, ...) are numbered in instantiation order over the whole circuit.

### Net names

`circuit.label(node, label)` names the net containing `node`:

| Label | Connects | Net name |
|---|---|---|
| `NetLabel::global("GND")` | every node with the same global name | `GND` |
| `NetLabel::local("OUT")` | nodes with the same local name in the same subcircuit instance | `/OUT`, `/amp_left/OUT` |
| (none) | - | `Net-(R1-Pad2)`, `Net-(U1-OUT)`, `unconnected-(U1-NC-Pad7)` |

- Sheet paths are made from the instance names of the subcircuits (`/amp_left/stage#1/`).
  `/` in names is written as `{slash}`.
- A net with several names is not an error (as in KiCad). The name is chosen by:
  global over local, then the label in the shallower subcircuit instance, then the smallest name.
  A `MultipleNetNames` warning is reported, and the other names still connect nets
  (a net labeled `ALPHA` and `ZED` is connected to another net labeled `ZED`).
- An unnamed net is named after one of its pins, as Eeschema does: `Net-(<ref>-<pin name>)`, or
  `Net-(<ref>-Pad<number>)` for a pin without a name or named as its pad number. A pin name shared
  by several pins of the part is followed by the pad (`Net-(U1-GND-Pad5)`). Pins written by their
  name alone are preferred, and among them the smallest name in string order (not natural order:
  `Net-(R10-Pad1)` before `Net-(R9-Pad1)`); the pin type does not matter. A net of a single pin is
  `unconnected-(...)`. Pins of parts without KiCad information are used only if no pin has it,
  by their pin IDs.
- A labeled net without any part pin is not written and gives a `LabelOnlyNet` warning.
- Two different nets with the same name are an error (`DuplicateNetName`).

## 4. Output

- `components`: `ref`, `value` (`~` if empty), `footprint`, `fields` (if any), `libsource` (if
  set), value-less `property` flags (`exclude_from_board`, `exclude_from_bom`, `dnp`), `sheetpath`
  and `tstamps`. Components are sorted by reference in natural order (`R2` before `R10`).
- `nets`: `code` (from 1), `name`, `class` (`"Default"`), and `node`s with `ref`, `pin`,
  `pinfunction` (if the pin has a name) and `pintype`. Nets are sorted by name in natural order
  (case-sensitive); nodes by reference and then pad, both in string order (as KiCad does).
- `design` has only `source` (if set with `with_source`) and `tool`. No date is written, so the
  output does not change between exports. `libparts` and `libraries` are not written.

## 5. Errors and warnings

Each error and warning carries a trace: the source location that caused it, followed by the
locations where the enclosing subcircuits were instantiated. `Display` prints them innermost first:

```
error: R3: no footprint is assigned
  at src/filters.rs:42:13
  in subcircuit instantiated at src/main.rs:18:5
```

| Error (`CircuitErrorKind`) | Raised by | Cause |
|---|---|---|
| `InvalidName` | `to_netlist` | empty name or label, or `#` in an explicit name |
| `DuplicateInstanceName` | `to_netlist` | the same explicit name twice in one (sub)circuit (the note points at the first) |
| `DuplicatePinId` / `UnknownPinId` | `to_netlist`, `to_kicad` | duplicate pin IDs; unknown pin IDs in internal connections or KiCad pin information |
| `DuplicateRef` | `to_netlist` | two parts with the same reference designator |
| `DuplicateNetName` | `to_netlist` | two nets with the same name |
| `KicadUnsupported` | `to_kicad` | `kicad()` returns `None` |
| `FootprintMissing` | `to_kicad` | no footprint assigned |
| `PinInfoMissing` | `to_kicad` | no `KicadPinInfo` for a pin ID |
| `DuplicatePad` | `to_kicad` | two pins on the same pad |

Warnings (`MultipleNetNames`, `LabelOnlyNet`) are printed to stderr and kept in `Netlist::warnings`.

## 6. Verifying the output

- **Golden tests**: `tests/golden/*.net` are expected outputs of small circuits defined in the unit
  tests. After an intended output change, regenerate them with `UPDATE_GOLDEN=1 cargo test` and
  review the diff. Do not add outputs generated from the KiCad demos (see the license notes in
  `doc/kicad_netlist_format.md`).
- **Footprints and pads**: `python3 tools/check_netlist_footprints.py NETLIST...` checks with KiCad's
  `pcbnew` Python module that every footprint exists in the installed libraries and every `pin` is a
  pad of it.
- **Import into pcbnew**: `python3 tools/check_netlist_import.py [--keep DIR] NETLIST...` imports
  each netlist with the real pcbnew GUI and checks the result. pcbnew's netlist reader is not
  available from Python or `kicad-cli`, so the script runs pcbnew on a virtual X server (Xvfb, installed
  in the devcontainer) with a throwaway KiCad configuration, and a scripting plugin
  (`tools/pcbnew_gui/import_netlist_plugin.py`) drives File -> Import -> Netlist... inside pcbnew.
  1. The netlist is imported into an empty board, matching footprints by UUID (tstamps). The report
     must have no errors; warnings are printed but do not fail the check.
  2. The saved board must match the netlist: references, footprints, values, symbol and sheet paths,
     fields, DNP / exclude-from-BOM flags, and the net, pin function and pin type of every pad.
  3. The footprints are moved and the same netlist is imported again: no footprint may be replaced
     or moved (UUIDs are stable).

  `--keep DIR` keeps the boards, the import reports, a screenshot of the dialog (`report.png`) and
  the pcbnew logs. In the Claude Code sandbox, Unix sockets cannot be created, so Xvfb listens on
  TCP (`127.0.0.1`), and its keymap directory `/var/lib/xkb` must be writable
  (`sandbox.filesystem.allowWrite` in `.claude/settings.json`).

  `--relink BEFORE AFTER` imports BEFORE, moves the footprints and imports AFTER, a revised
  circuit, checking that footprints are linked by UUID: footprints of components still in the
  netlist keep their places even if renumbered, and new components are added. The golden files
  `tests/golden/revision_*.net` are such revisions (a named part added before the others, and an
  automatically named part inserted before another one, which takes over its UUID):

  ```sh
  G=tests/golden
  python3 tools/check_netlist_import.py --relink \
      $G/revision_before.net $G/revision_named_added.net \
      $G/revision_before.net $G/revision_auto_inserted.net
  ```

  The IPC API of KiCad 9 and 10 has no netlist import; the `ImportNetlist` command and the headless
  `kicad-cli api-server` expected in KiCad 11 may allow a check without the GUI.
- **Comparison with Eeschema**: `python3 tools/compare_kicad_cli_netlist.py [--keep DIR] NETLIST...`
  synthesizes a schematic equivalent to each netlist (one symbol per component with the pins of its
  nodes, wires, and a global or local label for each named net; unnamed nets are left for KiCad to
  name), exports it with `kicad-cli sch export netlist`, and prints the differences in element
  order, quoting, omission rules and net names. What cuprite_wire intentionally omits (`libparts`,
  the `Footprint`/`Datasheet`/`Description` fields, properties repeating fields, ...) is ignored.
  Only flat netlists (every component on the root sheet) are supported.
