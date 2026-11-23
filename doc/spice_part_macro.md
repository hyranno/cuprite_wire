# Parts from SPICE Models

Vendor SPICE models (`.lib`, `.sub`, `.cir`, ...) can be used in two ways:

- **`spice_part!`** makes a simulation-only part from a `.subckt` or a `.model`. Its pins are the
  ports of the model; it cannot be exported to KiCad.
- **The `sim` block of `kicad_part!`** attaches a model to a part generated from a KiCad symbol,
  with the pins mapped to the ports by hand. It replaces the symbol's `Sim.*` fields, which vendor
  symbols often lack or get wrong.

```rust
use cuprite_wire::{kicad_part, spice_part};

spice_part! {
    /// TPS3899 voltage supervisor (simulation only).
    pub struct Tps3899Model {
        library: "models/TPS3899DL01.lib",     // relative to the crate, or ${VAR}/...
        name: "TPS3899DL01",
        pins: { "RESET_B" => "reset_n" },      // pin IDs are the port names otherwise
    }
}

kicad_part! {
    /// TPS3899DL01 in SOT-23-6 with TI's PSpice model.
    pub struct Tps3899Dl01Dse {
        symbol: "PTPS3899DL01DSER:PTPS3899DL01DSER",
        symbol_dir: "vendor/ti/tps3899",
        footprint: "DSE0006A:DSE0006A-MFG",
        footprint_dir: "vendor/ti/tps3899",
        pins: { "6" => "reset_n" },
        sim: {
            library: "vendor/ti/tps3899/TPS3899DL01.lib",
            name: "TPS3899DL01",
            // Symbol pin number => .subckt port (the direction of KiCad's Sim.Pins).
            pins: { "1" => "CTR", "2" => "CTS", "3" => "GND", "4" => "VDD", "5" => "SENSE", "6" => "RESET_B" },
        },
    }
}
```

The model library is read at compile time (with the files it includes) to check the mapping;
the body of the model is not interpreted, ngspice reads it. Files read are tracked for rebuilds.

## 1. Options

### `spice_part!`

| Key | Value | Default |
|---|---|---|
| `library` | the library file: relative to the crate, absolute, or with `${VAR}` (environment variables) | required |
| `name` | the `.subckt` or `.model` (case-insensitive) | required |
| `pins` | `{ "IN+" => "in_p", ... }`: pin IDs by port name | the port names, as written in the library |
| `internal_connections` | `[["a", "b"], ...]`: groups of pin IDs | none |
| `params` | `{ "TD" => "1m" }`: parameters of the `.subckt` | the model's defaults |
| `value` | `"text"` or `param` | the model name |
| `ref_prefix` | reference prefix (a letter, then letters, digits and `_`, not ending with a digit) | `U` for a `.subckt`; `D` for a diode, `Q` for other `.model`s |
| `dialect` | `auto`, `ngspice`, `pspice` or `ltspice` (section 4) | `auto` |
| `sim_model` | `include` or `inline` (as for `kicad_part!`) | `include` |

Port names such as `IN+` or `1` are not identifiers, so constants (`Tps3899Model::RESET_N`) are
made only for the pins renamed with `pins` (whose IDs must differ in more than case); `PINS` lists
all of them. `kicad()` returns `None`, and
`spice()` returns one element: `X` for a `.subckt`, `D` `Q` `M` `J` for a `.model` of type `D`,
`NPN`/`PNP`, `NMOS`/`PMOS`/`VDMOS`, `NJF`/`PJF` (terminals `A K`, `C B E`, `D G S [B]`, `D G S`).

### The `sim` block of `kicad_part!`

| Key | Value | Default |
|---|---|---|
| `library`, `name` | as for `spice_part!`; relative paths are relative to `sim_library_dir` | required |
| `pins` | `{ "1" => "CTR", ... }`: symbol pin number (or extra pad) => port | none |
| `unused` | `["7"]`: pins not connected to the model | none |
| `params`, `dialect` | as for `spice_part!` | |
| `suffix` | element suffix (`XU1A`), when there are several models | none |

- Pins are numbers here, before the renaming of the top-level `pins` (as for the other options
  but `internal_connections`).
- `sim` cannot be combined with `spice: none` / `spice: excluded`.
- **Several models**: `sim: [{ ..., suffix: "A" }, { ..., suffix: "B" }]`. Vendors often model a
  dual op-amp as one amplifier; use it once per unit. A pin may be mapped in several models (the
  supply pins); each model checks its own ports (section 3).
- The documentation of the generated type gets a "SPICE port" column in its pin table, and the
  model's file, dialect and parameters, so the mapping can be checked in the editor's hover.
- With a single model, the netlist (`.net`) gets the `Sim.Device`, `Sim.Library` (absolute),
  `Sim.Name`, `Sim.Pins` (and `Sim.Params`) fields KiCad would have, as fields of the component.

## 2. Mapping pins to ports

**Map by name, never by order.** The order of the pin numbers of a symbol has nothing to do with
the order of the ports on the `.subckt` line. In skidl discussion #293, TI's TPS3899DL01 symbol has
the pins 1–6 `CTR CTS GND VDD SENSE RESET_N`, while the model's ports are
`CTR CTS GND RESET_B SENSE VDD`: sorting either does not give the other, and `RESET_N` is called
`RESET_B` in the model. So the mapping is written by hand, as in KiCad's `Sim.Pins`, and the nodes
of the element always follow the `.subckt` line:

```
XU1 <pin 1> <pin 2> <pin 3> <pin 6> <pin 5> <pin 4> TPS3899DL01
```

Mappings are never derived automatically: a wrong mapping simulates, just wrongly. Errors suggest
mappings from the names (section 3) for you to check against the datasheet and copy.

## 3. Checks and errors

Mistakes are compile errors pointing at the option concerned:

1. The library has no such `.subckt` / `.model`. The error lists similar names, and the
   subcircuits no other definition uses (most likely the products of the library).
2. A pin is mapped to a port the model does not have, or a port is mapped twice.
3. A port of the model has no pin. The error shows the pins with their ports, and mappings
   suggested from the names (uppercase, without `~{}/#` and a trailing `_N` `_B` `N` `B`; `+` and
   `-` are kept):

   ```
   error: no pin is mapped to the ports RESET_B of subcircuit TPS3899DL01
          pins and ports:
            1 (CTR)          => CTR
            ...
            6 (~{RESET_N})   => -
          suggested from the names (not applied; check them against the datasheet):
            "6" => "RESET_B",
   ```

4. (`sim` block) A pin of the part is neither mapped, nor in `unused`, nor connected inside the
   part (stacked pins, `internal_connections`) to a mapped pin. Pins are not silently left out:
   that is where vendor mappings go wrong.
5. A parameter is not on the `.subckt` line (`PARAMS:` or `name=value`); the error lists them
   with their defaults. `.model`s take no parameters.
6. The model is encrypted (`$CDNENCSTART` ... `$CDNENCFINISH`), or has `OPTIONAL:` ports (PSpice
   digital parts); ngspice cannot use either.
7. The dialect is ambiguous or does not fit the library (section 4).

The symbol's own `Sim.*` fields go through the same checks, except 4: as in KiCad, pins missing
from `Sim.Pins` are unused.

## 4. Dialects and ngspice's compatibility modes

Vendor models are often written for PSpice (TI) or LTspice (Analog Devices). The model text is not
rewritten; ngspice's compatibility mode (`set ngbehavior=...`) reads it. The macros find the dialect
from what the library (and the files it includes) uses, and record it in the element's models
(`ModelSource::include(path).pspice()`):

| Construct | Default mode | PSpice mode |
|---|---|---|
| `.SUBCKT ... PARAMS: A=1`, `X... PARAMS: A=2`, `VALUE = { ... }`, `V(IN+,IN-)`, `.FUNC`, `TABLE`, `**` | ok | ok |
| `IF(c, a, b)`, single `&` / `|`, `STP(x)` | error | ok |
| `LIMIT(x, lo, hi)` | **silently 0** | ok |
| `.LIB "file"` (PSpice's include) | not read | read |
| `$` after whitespace | comment | not a comment |
| `.lib file section` / `.endl` | read | **broken** |
| `OPTIONAL:` ports, `AKO:` models, encrypted blocks | error | error |

(ngspice 44.2. LTspice's `uplim()` / `dnlim()` need the LTspice mode.)

- `dialect: auto` (default) chooses `pspice` if the library has PSpice constructs (`IF(`, `&`, `|`,
  `STP(`, `LIMIT(`, `.LIB file`), `ltspice` for `uplim(` / `dnlim(`, and `ngspice` otherwise;
  both kinds are an error. The documentation of the generated type says what was found.
  Give the dialect explicitly to override a wrong guess.
- `.LIB file` in a library read with `sim_model: include` needs `dialect: pspice`; a library using
  `.lib file section` cannot be included in the PSpice mode (use `sim_model: inline`).
- The netlist collects the dialects of its models (`SpiceNetlist::ngbehavior()`): `ps`, `lt` or
  `ltps`, with `a` appended (`psa`) if such a model is written into the deck (`sim_model: inline`).
  Without `a`, ngspice translates only the included files, so the rest of the deck keeps ngspice's
  own syntax; with it, the whole deck. Other decks run in the default mode.
- `SpiceDeck::run` sets the mode for the run: see "Compatibility mode" in
  `doc/spice_netlist_usage.md` section 5.
- KiCad 9 sets the mode in the simulator settings ("Compatibility mode", e.g. "PSpice" or
  "PSpice and LTSpice"). Its modes are `psa`, `lta`, `ltpsa` and `hsa` (strings of KiCad 9's
  eeschema), always for the whole netlist. Choose the matching mode there to simulate the schematic
  with the same models.

## 5. XSPICE code models

ngspice's XSPICE code models (`A` elements such as `d_state`, `adc_bridge` and `dac_bridge`) can
be used inside a `.subckt`, which is then the model of the part like any other subcircuit (KiCad's
demo `simulation/up-down-counter` is such a library). A `.model` of a code model cannot be a part
by itself: wrap it in a `.subckt` with the ports the part needs.

Some code models read a file named by a parameter:

| Model type | Parameter |
|---|---|
| `d_state` | `state_file` |
| `d_source` | `input_file` |
| `d_process` | `process_file` |
| `d_cosim` | `simulation` |
| `filesource` | `file` |
| `table2D`, `table3D` | `file` |

ngspice opens these files relative to the directory it runs in (a temporary directory for
`SpiceDeck::run`), not to the library. The macros resolve a relative path against the library file
the `.model` is in (as ngspice does for `.include`), check that the file exists and track it for
rebuilds. Then:

- With `sim_model: inline`, the copied definition gets the absolute path.
- With `sim_model: include` (the default), the library is included as it is, so a relative path is
  a compile error suggesting `sim_model: inline`. Absolute paths in the library are fine.

The file itself is not copied: ngspice reads it when the deck runs, as it reads an included
library. For digital models built by your crate, `spice::xspice::DCosim` is simpler than a
`d_cosim` in a library (`doc/cosimulation.md` section 6).

## 6. What is not supported

- **Encrypted models** (PSpice's `$CDNENCSTART` blocks). Ask the vendor for an unencrypted model,
  or use a generic model.
- **`OPTIONAL:` ports** and PSpice digital parts (`U` devices, `$G_DPWR` / `$G_DGND`). If an analog
  model has them only for its supply, a copy with the optional ports made regular can be used.
- **Model bodies are not checked**: syntax ngspice cannot read even in its compatibility mode is
  reported when the deck runs (`SimulationError::Ngspice` with the log).
- Verilog-A, the `.model` of an XSPICE code model as a part by itself (wrap it in a
  `.subckt`, section 5) and TINA-TI's own formats.
- Parameters as constructor arguments (`params` are fixed values).
- IBIS models are not SPICE libraries: use `ibis_part!` (`doc/ibis_part_macro.md`).
- Mappings taken from vendor symbol files (`.olb`) or from header comments
  (`* PINOUT ORDER ...`), which follow no common format.

**Convergence options.** Vendor models may need options for ngspice to converge. Parts do not
set options; add them in the test bench, e.g. `netlist.deck().directive(".options rshunt=1e12")`.
With TI's LMx58/LM2904 model, the operating point fails without it (gmin and source stepping), and
the transient operating point ngspice falls back to leaves the output above the supply, without
any error: check results against what the circuit can do. Transient models (e.g. TI's TPS3899
model) are meant for `.tran`, as in the vendor's test bench.

## 7. Licenses of vendor models

Vendor models are usually free to use but restrict redistribution. Keep them out of your
repository unless their terms allow it, and prefer `sim_model: include` (the default), which only
writes the path of the library into the code and the deck. `sim_model: inline` copies the
definitions into your code. Give the path with an environment variable
(`library: "${VENDOR_MODELS}/TPS3899DL01.lib"`) to keep it out of the source.

cuprite_wire's own tests follow this: the model in `crates/kicad_lib/tests/fixtures/models/vsup01.lib`
is written for the project in the style of TI's models (CRLF, a Windows-1252 comment, `.LIB`,
`PARAMS:`, `IF(...)` with `&`). Tests with real models are opt-in (`doc/dev/verifying.md` section 1;
`tests/vendor_models.rs` lists the files it expects: TI's TPS3899DL transient model and LMx58/LM2904
model, checked with ngspice 44.2 on 2026-11-18). The TPS3899 model is marked "All rights reserved",
so it is only read from the directory given by `CUPRITE_VENDOR_MODELS_DIR` (e.g. the git-ignored
`vendor_models.local/`).

## 8. Verifying

How the model handling is tested is described in `doc/dev/verifying.md`.
