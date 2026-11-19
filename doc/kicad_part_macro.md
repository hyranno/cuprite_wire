# Generating Parts from KiCad Libraries

`kicad_part!` generates a part type (a struct implementing `Part`) from a KiCad 9 symbol and
footprint. The libraries are read at compile time, and their contents are written into the
generated code, so programs using the parts do not read the libraries at run time.

```rust
use cuprite_wire::kicad_part;

kicad_part! {
    /// A dual op-amp in SOIC-8.
    pub struct Lm358Soic {
        symbol: "Amplifier_Operational:LM358",
        footprint: "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm",
    }
}

kicad_part! {
    pub struct Resistor0603 {
        symbol: "Device:R",
        footprint: "Resistor_SMD:R_0603_1608Metric",
        value: param,
    }
}

let amp = circuit.instantiate_part(Box::new(Lm358Soic::new()));
let r = circuit.instantiate_part(Box::new(Resistor0603::new("10k")));
circuit.connect(amp["1"], r["1"]);
```

Ready-made parts generated from the MIT-licensed library in `libs/` are in `cuprite_wire::basic`
(section 7); they need no KiCad installation.

## 1. Options

Only `symbol` is required. Options are separated by commas.

| Key | Value | Default |
|---|---|---|
| `symbol` | `"Library:Name"` | |
| `footprint` | `"Library:Name"`, or `none` for simulation-only parts | the symbol's `Footprint` property; an error if it is empty |
| `value` | `"text"`, or `param` to take it as a constructor argument | the symbol's `Value` |
| `ref_base` | the reference prefix, e.g. `"RN"` | the symbol's `Reference` without `?` |
| `pins` | `{ "1" => "out_a", ... }`: pin IDs by pin (or extra pad) number | the pin numbers |
| `internal_connections` | `[["4", "ep"], ...]`: groups of pin IDs connected inside the part | stacked pins only |
| `extra_pads` | `["9"]`: footprint pads without a symbol pin, made pins of the part | none |
| `spice` | `auto`, `none` (`spice()` returns `None`) or `excluded` (excluded from the simulation) | `auto` |
| `sim` | a library model mapped to the pins by hand, replacing the symbol's `Sim.*` fields (section 4) | the `Sim.*` fields |
| `sim_model` | `include` or `inline` (section 4) | `include` |
| `fields` | `{ "MPN" => "..." }`: fields written to the footprint | none |
| `symbol_dir` / `footprint_dir` | library directories, relative to the crate (`CARGO_MANIFEST_DIR`) | `$KICAD9_SYMBOL_DIR` / `$KICAD9_FOOTPRINT_DIR`, else `/usr/share/kicad/symbols` / `footprints` |
| `sim_library_dir` | base of relative `Sim.Library` / `sim` library paths, relative to the crate | the crate |

A library nickname is the file name: `Library:Name` is the symbol `Name` in
`<symbol dir>/Library.kicad_sym`, and the footprint `<footprint dir>/Library.pretty/Name.kicad_mod`.
Library tables (`sym-lib-table`, `fp-lib-table`) are not read. The `libsource` and `footprint`
written to the netlist are these `Library:Name` identifiers, so pcbnew needs a footprint library
with the same nickname.

## 2. The generated type

- `pub struct Name;` with `const fn new()` and `Default`, or `pub struct Name { value }` with
  `fn new(value: impl Into<String>)` for `value: param`. It derives `Debug`, `Clone`, `PartialEq` and `Eq`.
- `Name::PINS`: the pin IDs. Each pin renamed with `pins` also gets a constant (`out_a` -> `Name::OUT_A`).
- The documentation of the type gets the symbol, the footprint, the symbol's description and
  datasheet, and a table of the pins (ID, pad, name, electrical type, unit), shown by rust-analyzer.
- `impl Part`: the reference prefix, the pins, the value, the internal connections, `kicad()`
  (footprint, pin numbers / names / types, `lib_source`, `fields`, and the `in_bom` / `on_board`
  flags of the symbol), and `spice()` (section 4).

## 3. Pins and pads

- **Pin IDs are the pin numbers** (`"1"`, `"A3"`), so one pin ID is one pad, as for hand-written
  parts. Pin names are not used as IDs: they are often `~`, `+`, `-`, or shared by several pins.
  Use `pins` to give readable IDs.
- Every unit of the symbol is part of the one generated part (the netlist has one component).
  Pins of unit 0 are shared by all units. A pin number defined in several units is one pin
  (an error if their names or types differ). Only body style 1 is read (De Morgan styles repeat the pins).
- **Stacked pins** (several pins at the same position of a unit) are connected inside the part,
  as KiCad connects them in a schematic.
- Symbols derived with `extends` take the pins of their base symbol and its properties overridden
  by their own.
- Every pin number must be a pad of the footprint. **Footprint pads without a symbol pin** (exposed
  pads, shields) are an error unless listed in `extra_pads`: an exposed pad that is forgotten is
  left unconnected. A listed pad becomes a pin (ID: its number, or the name given with `pins`) of
  electrical type `passive`, which can be connected with `internal_connections` (e.g. to the GND pin)
  or in the circuit. Pads without a number (paste apertures, holes) are ignored.
  The netlist then has a pad that is not a pin of the symbol, which a KiCad schematic cannot
  produce; pcbnew imports it and connects the pad (checked with
  `tools/check_netlist_import.py tests/golden/kicad_libraries_ep.net`).
- Not supported (compile errors): power symbols (`power:GND`; use `NetLabel::global`), and hidden
  `power_in` pins, which KiCad connects to a global net named after the pin.

## 4. Simulation (`spice()`)

With `spice: auto`, the SPICE description is derived from the symbol, following KiCad 9:

| Symbol | Element |
|---|---|
| `exclude_from_sim` or `Sim.Enable` `0` | `SpicePartInfo::Excluded` |
| no `Sim.*` fields, reference `R`, `C` or `L`, two pins | inferred: the value as is (`value` must be a number, or `param`) |
| no `Sim.*` fields, other references | no model: `spice()` returns `None` |
| `Sim.Device` `R` `C` `L` | the value, or `Sim.Params` `r=` / `c=` / `l=` |
| `Sim.Device` `V` `I` with `Sim.Type` `DC` | `DC <value>`, or `Sim.Params` `dc=` |
| `Sim.Device` `D`, `NPN` `PNP` (Gummel-Poon), `NMOS` `PMOS` (`VDMOS`, `MOS1`), `NJFET` `PJFET` (Shichman-Hodges) | a built-in model `.model __<TypeName> <type> <Sim.Params>`, or the `.model` `Sim.Name` of `Sim.Library` |
| `Sim.Device` `SUBCKT` with `Sim.Library` and `Sim.Name` | `X` element with the `.subckt` and `Sim.Params` as parameters |

- The device letter comes from the model (`Q` for `NPN`, `M` for `NMOS`, `X` for `SUBCKT`), and the
  element name gets it prepended when the reference does not start with it (`XU1`, `MQ2`), as in KiCad.
- Nodes follow the model's pin order (D: A K, Q: C B E, M: D G S [B], J: D G S, two-terminal: + -,
  a subcircuit: its `.subckt` line), mapped from the symbol pins by `Sim.Pins` (`1=K 2=A`), or in pin
  order without it. Symbol pins not in `Sim.Pins` (and extra pads) are declared unused.
- `Sim.Library` paths: `${KICAD9_SYMBOL_DIR}` (and `${KICAD<n>_SYMBOL_DIR}`) is the symbol directory
  used, other `${VAR}` are environment variables, and relative paths are relative to
  `sim_library_dir`. The library is read (with the files it `.include`s) to find the model and to
  check the pin mapping against the `.subckt` ports and the model type against `Sim.Device`.
- `sim_model: include` (default) writes `.include "<absolute path>"` in the deck.
  `sim_model: inline` copies the definition, and the top-level definitions it refers to by name,
  into the generated code, so the deck does not depend on the library file. **Check the license of
  the model library before inlining it** (section 6): vendor models often restrict redistribution.
- Anything else (other `Sim.Device` / `Sim.Type`, `Sim.Params` with expressions, IBIS, XSPICE) is a
  compile error. Choose `spice: none` (not simulated) or `spice: excluded` explicitly.
- Built-in models are named after the generated type (`__Diode`), while KiCad names them after the
  reference (`__D1`); the definitions are the same.
- The library model is checked as described in `doc/spice_part_macro.md` section 3 (except that
  pins missing from `Sim.Pins` are unused, as in KiCad), and its dialect is detected: models written
  for PSpice or LTspice run in ngspice's compatibility mode (`doc/spice_part_macro.md` section 4).

**The `sim` block.** When the symbol has no (or wrong) `Sim.*` fields, e.g. a vendor's symbol,
give the model and the mapping in the macro. The keys are those of `Sim.*`, with the pins mapped
by number to the `.subckt` ports, as in `Sim.Pins`:

```rust
kicad_part! {
    pub struct Vsup {
        symbol: "test:VSUP",
        sim: {
            library: "models/vsup01.lib",
            name: "VSUP01",
            pins: { "1" => "CTR", "2" => "CTS", "3" => "GND", "4" => "VDD", "5" => "SENSE", "6" => "RESET_B" },
            // unused: ["7"], params: { "VTH" => "0.6" }, dialect: pspice,
        },
    }
}
```

Unlike `Sim.Pins`, every pin must be mapped, listed in `unused`, or connected inside the part to a
mapped pin. A list of models with suffixes (`sim: [{ ..., suffix: "A" }, ...]`) uses a
single-amplifier model for each unit. See `doc/spice_part_macro.md` for the keys, the checks and
the dialects, and for `spice_part!`, which makes a part from a model alone.

## 5. Rebuilds and errors

- Every file read and environment variable consulted is tracked (`include_bytes!` / `option_env!`
  in the generated code), so changing a library or `KICAD9_SYMBOL_DIR` rebuilds the parts.
- Errors are compile errors pointing at the option concerned, e.g.:

  ```
  error: footprint test:SOIC-8-EP has pads without a symbol pin: 9; list them in `extra_pads` (and connect them, e.g. with `internal_connections`)
   --> src/parts.rs:5:20
    |
  5 |         footprint: "test:SOIC-8-EP",
    |                    ^^^^^^^^^^^^^^^^
  ```

- The library reading is in the crate `cuprite_wire_kicad_lib` (`crates/kicad_lib`), which does
  not depend on `proc_macro`; the macro (`crates/macros`) only parses the options and writes the code.

## 6. Licenses of libraries

cuprite_wire is MIT licensed. Libraries used with the macro keep their own licenses:

- **KiCad's libraries** (symbols, footprints, `Simulation_SPICE.sp`) are CC-BY-SA 4.0 with an
  exception that waives the attribution and share-alike conditions for designs using them and for
  "generated files which use data provided as part of the Licensed Material". Code generated by the
  macro and netlists of circuits using the parts are such generated files, as far as we understand it;
  the library files themselves, or excerpts of them, are not. cuprite_wire therefore never copies
  library files into the repository: the macro reads them at compile time, the test fixtures are
  written for this project, and only netlists of circuits using KiCad parts are committed
  (`tests/golden/kicad_libraries*`).
- **Vendor SPICE models** have their own terms. `sim_model: include` (the default) does not copy
  them; `sim_model: inline` does, into your code.
- **Other libraries** (e.g. third-party KiCad libraries under MIT) can be used with `symbol_dir` /
  `footprint_dir`. Check where their data comes from as well as their license.

## 7. The basic library (`libs/`, `cuprite_wire::basic`)

`libs/` holds a small library made for this project under its MIT license: resistor, capacitor and
inductor (0603), diode (SOD-123), LED (0603), single op-amp (SOT-23-5) and dual op-amp (SOIC-8),
with SPICE models (`libs/symbols/cuprite_basic.sp`: a generic diode and LED, and a single-pole
op-amp with a gain of 100k and a gain-bandwidth of 1 MHz, without supply rails).
It is generated by `tools/generate_basic_library.py` from the common pinouts of the packages and
nominal dimensions; nothing is copied from the KiCad libraries. Regenerate it after editing the script.

`cuprite_wire::basic` has the parts generated from it (`Resistor0603`, `Capacitor0603`,
`Inductor0603`, `DiodeSod123`, `Led0603`, `OpAmpSot23`, `DualOpAmpSoic8`; all take the value as a
constructor argument, the op-amps' models are inlined). To import their netlists into pcbnew, add
`libs/footprints/cuprite_basic.pretty` to the footprint library table as `cuprite_basic`
(the check scripts in `tools/` do this themselves).

## 8. Verifying generated parts

- `cargo test --features kicad-libraries --test kicad_libraries` generates parts from the installed
  KiCad libraries, compares the netlists of a circuit using them with `tests/golden/kicad_libraries.*`,
  and simulates it with KiCad's models.
- `python3 tools/compare_kicad_cli_netlist.py --library-symbols [--spice] NETLIST...` builds a
  schematic from the components' library symbols (with their `Sim.*` fields for `--spice`, replaced by
  those of the netlist for parts with a `sim` block) and compares KiCad's export with ours: pin
  numbers, names and types, and the SPICE elements KiCad derives from the symbols.
  `tests/golden/basic_parts.net`, `tests/golden/kicad_libraries.net` and (with
  `--symbol-dir crates/kicad_lib/tests/fixtures/symbols`) `tests/golden/spice_part.net` are the same
  as KiCad's.
- `python3 tools/check_netlist_footprints.py` and `tools/check_netlist_import.py` (see
  `doc/kicad_netlist_usage.md`) also find the footprints of `libs/` and `examples/*/libs/`.
