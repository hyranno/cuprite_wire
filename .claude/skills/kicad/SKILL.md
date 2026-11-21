---
name: kicad
description: Investigate KiCad 10 data and behavior from the CLI - look up symbols/footprints/pins in the installed libraries, export reference netlists (KiCad S-expr .net, SPICE) and run ERC on schematics with kicad-cli, and read the demo projects. Use when researching KiCad file formats, part pinouts, footprint pads, netlist output, or ERC rules.
---

# KiCad investigation

KiCad 10.0 is installed in the devcontainer (`kicad-cli`, Python module `pcbnew`; image
`kicad/kicad:10.0`). CI also tests with KiCad 9 (Debian's libraries), which is not installed here.
This skill is for *research*: the project (cuprite_wire) generates netlists from Rust
and does not produce `.kicad_sch` / `.kicad_pcb`.

## Running kicad-cli inside the Claude sandbox

`$HOME` is read-only in the bash sandbox, so KiCad prints
`Directory '/root/.config/kicad' couldn't be created` and may misbehave.
Redirect its config/cache/data dirs to `$TMPDIR` first:

```bash
export XDG_CONFIG_HOME=$TMPDIR/kicad/config XDG_CACHE_HOME=$TMPDIR/kicad/cache XDG_DATA_HOME=$TMPDIR/kicad/data
```

Write all outputs under `$TMPDIR` (or the scratchpad), never next to the input files
(`/usr/share/kicad` is read-only).

## Where things are

| What | Path |
|---|---|
| Symbol libraries (`LIB.kicad_sym`) | `/usr/share/kicad/symbols` (`$KICAD10_SYMBOL_DIR`) |
| Footprint libraries (`LIB.pretty/NAME.kicad_mod`) | `/usr/share/kicad/footprints` |
| Demo projects (real schematics) | `/usr/share/kicad/demos` |
| Simulation demos (Sim.* fields, `.lib` models) | `/usr/share/kicad/demos/simulation` |
| SPICE-only symbols / built-in subcircuits | `symbols/Simulation_SPICE.kicad_sym`, `symbols/Simulation_SPICE.sp` |
| File format docs | not shipped; see https://dev-docs.kicad.org/en/file-formats/ |

## Library lookup helper

`scripts/kicad_lib.py` (stdlib only; run with `python3 -I`):

```bash
K=.claude/skills/kicad/scripts/kicad_lib.py
python3 -I $K search 'LM35[0-9]' --lib Amplifier        # symbols by name regex
python3 -I $K search 'opamp' --desc --limit 20          # also match Description/keywords
python3 -I $K search '^SOIC-8_' --fp                    # footprints by name regex
python3 -I $K sym Amplifier_Operational:LM358           # properties, units, pins (extends resolved)
python3 -I $K fp Package_SO:SOIC-8_3.9x4.9mm_P1.27mm    # pads
python3 -I $K libs [--fp]                               # library nicknames
```

Notes on symbol data:
- `(extends "PARENT")` - derived symbols only override properties; pins/graphics come from the parent.
- Sub-symbols are named `NAME_<unit>_<bodystyle>`; unit `0` holds items shared by all units.
  Multi-unit parts (e.g. LM358) often have a separate power unit.
- Pin electrical types: `input output bidirectional tri_state passive free unspecified
  power_in power_out open_collector open_emitter no_connect`.
- `ki_fp_filters` lists glob patterns of compatible footprints; the netlist pin `num`
  must match a footprint pad number.
- Power symbols (`power:GND` etc.) have `(power)`, reference `#PWR`, and a hidden
  `power_in` pin; they create a global net named by their Value.

## Reference outputs from kicad-cli

Use a demo schematic to see exactly what KiCad emits, then diff against our output.

```bash
S=/usr/share/kicad/demos/simulation/sallen_key/sallen_key.kicad_sch
mkdir -p $TMPDIR/ref
kicad-cli sch export netlist --format kicadsexpr -o $TMPDIR/ref/out.net $S   # pcbnew netlist
kicad-cli sch export netlist --format spice      -o $TMPDIR/ref/out.cir $S   # SPICE netlist
kicad-cli sch export netlist --format spicemodel -o $TMPDIR/ref/out.lib $S   # as .subckt
kicad-cli sch erc --format json --severity-all   -o $TMPDIR/ref/erc.json $S  # ERC report
kicad-cli sch export bom -o $TMPDIR/ref/bom.csv $S
kicad-cli sym export svg -o $TMPDIR/ref/svg -s LM358 /usr/share/kicad/symbols/Amplifier_Operational.kicad_sym
```

Other netlist formats: `kicadxml cadstar orcadpcb2 pads allegro`.
`kicad-cli` cannot read a `.net` back; importing a netlist into pcbnew is GUI-only
(the `pcbnew` Python module has no netlist reader).

### KiCad S-expr netlist (`.net`, version "E") essentials
- `(export (version "E") (design ...) (components (comp ...)...) (libparts ...) (libraries ...) (nets (net ...)...))`
- `comp`: `(ref "R1") (value "1k") (footprint "Resistor_SMD:R_0603_1608Metric") (libsource (lib ..) (part ..)) (tstamps "<uuid>")`.
  pcbnew matches components to footprints by `tstamps` (path), falling back to reference.
- `net`: `(net (code "1") (name "/VIN") (class "Default") (node (ref "R1") (pin "1") (pintype "passive")) ...)`.
  Hierarchical local nets are prefixed with the sheet path (`/`); auto names look like `Net-(R1-Pad2)`.
- Look at a real export for details before guessing; the format is undocumented in the install.

## ERC reference

- Rules and default severities: `erc.rule_severities` in any `.kicad_pro`
  (e.g. `demos/simulation/sallen_key/sallen_key.kicad_pro`).
- Pin-to-pin conflict matrix: `erc.pin_map`, a 12x12 matrix (0 = OK, 1 = warning, 2 = error)
  indexed in this order:
  `input, output, bidirectional, tri_state, passive, free, unspecified, power_in, power_out,
  open_collector, open_emitter, no_connect`.
- Notable netlist-level checks: `pin_not_connected`, `pin_not_driven`, `power_pin_not_driven`,
  `pin_to_pin`, `duplicate_reference`, `different_unit_net`, `different_unit_footprint`,
  `unit_value_mismatch`, `no_connect_connected`.
- To observe a rule, run `kicad-cli sch erc --format json` on a demo and read `violations[].type`.

## pcbnew Python module

`python3 -c "import pcbnew"` works (KiCad 10.0). Useful for reading footprints with
geometry resolved, e.g. `pcbnew.FootprintLoad('/usr/share/kicad/footprints/Package_SO.pretty', 'SOIC-8_3.9x4.9mm_P1.27mm')`
then `fp.Pads()`. Set the XDG variables above first.
