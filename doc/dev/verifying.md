# Verifying the Output

This document describes how cuprite_wire's output is checked against KiCad and ngspice: the tests
run by `cargo test`, and the scripts in `tools/` that drive KiCad. The commands CI runs are in
`CONTRIBUTING.md`.

## 1. Tests

- **Golden files**: `tests/golden/*.net` and `tests/golden/*.cir` are the expected KiCad netlists
  and SPICE decks of small circuits defined in the tests. After an intended output change,
  regenerate them with `UPDATE_GOLDEN=1 cargo test` and review the diff. Do not add outputs
  generated from the KiCad demos (see the license notes in `doc/dev/kicad_netlist_format.md`).
  Golden files write the repository as `${CARGO_MANIFEST_DIR}`, which the tools below set for KiCad.
- **Simulations**: `src/spice/tests.rs` checks analyses against hand-calculated results
  (RC corner frequency and time constant, a divider with an ammeter, an amplifier with a
  subcircuit model). They need `ngspice` (`doc/spice_netlist_usage.md` section 5).
  The CLI copies its input with glibc `tmpfile()`, which always writes to `/tmp` and ignores
  `$TMPDIR`; the devcontainer's `ngspice` wrapper (`.devcontainer/tmpfile_shim.c`) redirects it
  to `$TMPDIR`, so the tests also run where `/tmp` is read-only, such as the Claude Code sandbox.
- **KiCad libraries**: `cargo test --features kicad-libraries --test kicad_libraries` generates
  parts from the installed KiCad libraries, compares the netlists of a circuit using them with
  `tests/golden/kicad_libraries.*`, and simulates it with KiCad's models.
- **SPICE models**: `tests/spice_part.rs` generates parts from the test model, checks that the
  nodes follow the `.subckt` order rather than the pin numbers, and simulates the supervisor in the
  PSpice mode: swapping two pins in the mapping changes the result, and without the PSpice mode
  ngspice rejects the model.
- **XSPICE code models**: `tests/xspice_models.rs` simulates a counter made of a `d_state` machine
  in a `.subckt` (`crates/kicad_lib/tests/fixtures/models/xspice/`), whose state file is named by
  a relative path, made absolute by `sim_model: inline`.
- **IBIS files**: `crates/ibis/tests/parse.rs` reads a hand-written file
  (`crates/ibis/tests/fixtures/buffers.ibs`) covering the keywords the parser reads and skips,
  and, as an ignored test (`cargo test -p cuprite_wire_ibis -- --ignored`), the IBIS file of the
  KiCad demo `simulation/ibis`.
- **IBIS simulation**: `crates/ibis/tests/spice.rs` simulates the generated subcircuits against
  hand-calculated results (the I-V tables, clamps, terminations, series models, edges) and checks
  that the weights solved from recorded waveforms drive another load as the original model does.
  Its ignored tests (`cargo test -p cuprite_wire_ibis -- --ignored`, run by the KiCad 10 job of CI)
  compare the drivers with KiCad's own IBIS conversion of the demo `simulation/ibis`: the weights
  and the pin voltage. `src/spice/ibis/tests.rs` simulates the parts of `spice::ibis`, and the tests
  of `examples/ibis_signal_integrity/` run the whole path from a `DigitalSequence` through the
  buffers and traces to `decode`. IBIS files of real devices: `tests/vendor_ibis.rs` (opt-in like
  the vendor models below; the files and the checks are listed in the test).
- **Vendor models** (opt-in, not run by CI):
  `CUPRITE_VENDOR_MODELS_DIR=... cargo test --features vendor-models --test vendor_models`
  (`doc/spice_part_macro.md` section 7).
- **Compile errors of the macros**: `tests/kicad_part_errors.rs` (trybuild); regenerate the
  messages with `TRYBUILD=overwrite cargo test --test kicad_part_errors`.
- **S-expression helpers of the tools**: `python3 -m doctest tools/kicad_netlist.py` checks the
  quoting and unescaping shared by the scripts in `tools/` (not run by CI).

The scripts in `tools/` work with KiCad 9 and 10. They take the version from `kicad-cli --version`
and the library directories from `$KICAD10_SYMBOL_DIR` / `$KICAD9_SYMBOL_DIR` (and `FOOTPRINT`,
`TEMPLATE`), else `/usr/share/kicad`.

## 2. Footprints and pads

`python3 tools/check_netlist_footprints.py NETLIST...` checks with KiCad's `pcbnew` Python module
that every footprint exists in the installed libraries (or in `libs/` and `examples/*/libs/`) and
every `pin` is a pad of it.

## 3. Import into pcbnew

`python3 tools/check_netlist_import.py [--keep DIR] NETLIST...` imports each netlist with the real
pcbnew GUI and checks the result. pcbnew's netlist reader is not available from Python or
`kicad-cli`, so the script runs pcbnew on a virtual X server (Xvfb, installed in the devcontainer)
with a throwaway KiCad configuration, and a scripting plugin
(`tools/pcbnew_gui/import_netlist_plugin.py`) drives File -> Import -> Netlist... inside pcbnew.
The libraries of `libs/` and `examples/*/libs/` are added to its library tables.

1. The netlist is imported into an empty board, matching footprints by UUID (tstamps). The report
   must have no errors; warnings are printed but do not fail the check.
2. The saved board must match the netlist: references, footprints, values, symbol and sheet paths,
   fields, DNP / exclude-from-BOM flags, and the net, pin function and pin type of every pad.
3. The footprints are moved and the same netlist is imported again: no footprint may be replaced
   or moved (UUIDs are stable).

`--keep DIR` keeps the boards, the import reports, a screenshot of the dialog (`report.png`) and
the pcbnew logs. In the Claude Code sandbox, Unix sockets cannot be created, so Xvfb listens on
TCP (`127.0.0.1`), and its keymap directory `/var/lib/xkb` must be writable
(`sandbox.filesystem.allowWrite` in `.claude/settings.json`). KiCad 10 loads the libraries in the
background (a progress bar in the status bar), so the plugin waits for it before importing;
otherwise footprints are "not found in any libraries" on a fresh configuration.

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

## 4. Comparison with Eeschema

`python3 tools/compare_kicad_cli_netlist.py [--keep DIR] NETLIST...` synthesizes a schematic
equivalent to each netlist, exports it with `kicad-cli`, and compares the result with ours.

- **KiCad netlist** (default): one symbol per component with the pins of its nodes, wires, and a
  global or local label for each named net; unnamed nets are left for KiCad to name. It is exported
  with `kicad-cli sch export netlist`, and the differences in element order, quoting, omission
  rules and net names are printed. What cuprite_wire intentionally omits (`libparts`, the
  `Footprint`/`Datasheet`/`Description` fields, properties repeating fields, ...) is ignored.
  Each subcircuit instance becomes a sheet; local nets reaching other sheets are connected through
  hierarchical labels. Two cases are not checked: an unnamed net crossing sheets (Eeschema needs a
  label to connect sheets, so it is connected by global labels named after it), and the choice
  among several labels of one net (only the label that named the net is in the netlist).
  cuprite_wire writes the KiCad 9 form of the format, which KiCad 10 reads; with KiCad 10, what
  only its form has is ignored too: the `units` of components, the empty `groups` and `variants`,
  and the `_<pin>` suffix of `pinfunction` (KiCad 10 writes `K_1` for the pin `K` numbered `1`).
- **SPICE** (`--spice`): takes a KiCad netlist and the SPICE deck of the same circuit
  (`NETLIST.cir`), adds `Sim.*` fields from the deck to the synthesized schematic (`Sim.Pins` maps
  the pads to the element's nodes by their nets), exports it with
  `kicad-cli sch export netlist --format spice`, and compares the element lines. KiCad writes the
  ground as `GND` (mapped to `0` for the comparison) and orders elements differently (reported, not
  a difference). Only `R`, `C`, `L` and `X` elements with an inline `.subckt` are supported.
  `tests/golden/kicad_pair.net` / `kicad_pair.cir` is such a pair.
- **Library symbols** (`--library-symbols`, with or without `--spice`): builds the schematic from
  the components' library symbols (with their `Sim.*` fields for `--spice`, replaced by those of
  the netlist for parts with a `sim` block) and compares pin numbers, names and types, and the SPICE
  elements KiCad derives from the symbols. `--symbol-dir DIR` adds a symbol library directory.
  `tests/golden/basic_parts.net`, `tests/golden/kicad_libraries.net` and (with
  `--symbol-dir crates/kicad_lib/tests/fixtures/symbols`) `tests/golden/spice_part.net` are the
  same as KiCad's.
