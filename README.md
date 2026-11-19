# cuprite_wire

Describe electronic circuits in Rust, test them with ERC and SPICE simulations, and hand them to
the KiCad 9 PCB editor (pcbnew) as a netlist.

```rust
use cuprite_wire::{basic::{Capacitor0603, OpAmpSot23, Resistor0603}, circuit::CircuitBuilder};

// An RC low-pass filter followed by a voltage follower.
let mut c = CircuitBuilder::new();
let r1 = c.instantiate_part_named("r1", Box::new(Resistor0603::new("10k")));
let c1 = c.instantiate_part_named("c1", Box::new(Capacitor0603::new("100n")));
let amp = c.instantiate_part_named("amp", Box::new(OpAmpSot23::new("LMV321")));
c.connect(r1["2"], c1["1"]);
c.connect(r1["2"], amp[OpAmpSot23::IN_P]);
c.connect(amp[OpAmpSot23::OUT], amp[OpAmpSot23::IN_N]);
```

## What it does

- **Circuits as code.** Parts, nets, labels and reusable subcircuits are ordinary Rust values, so a
  board can use functions, loops, generics, modules, documentation comments, version control and
  code review like any other Rust code.
- **Parts from libraries.** `kicad_part!` generates a part type from a KiCad symbol and footprint
  at compile time and checks the pins against the pads; `spice_part!` and the `sim` block attach
  vendor SPICE models. `cuprite_wire::basic` has ready-made resistors, capacitors and so on.
- **ERC.** The electrical rules check of KiCad 9 that depend only on the netlist, on a whole board
  or on a subcircuit alone with its ports given pin types. Violations point to the line of the
  Rust source that made the connection.
- **SPICE simulation.** A subcircuit is placed in a test bench with simulation-only sources and
  simulated with ngspice from `cargo test`. Vendor models written for PSpice or LTspice are read
  as they are, through ngspice's compatibility modes.
- **KiCad netlist.** The board is exported as a KiCad S-expression netlist (`.net`) that pcbnew
  imports directly. The output is deterministic.

`examples/sensor_frontend/` shows all of this on a small board: an analog front end made of
subcircuits, each with its own ERC and SPICE tests, checked as a whole and exported to pcbnew.

```sh
cargo run --example sensor_frontend > sensor_frontend.net   # ERC, then the netlist
cargo test --example sensor_frontend                          # ERC and SPICE tests of the subcircuits
```

## Why

KiCad's schematic editor (eeschema) is an excellent tool, and cuprite_wire relies on KiCad's
libraries, its rules and pcbnew. Still, some boards are easier to write as code than to draw:
repeated or parameterized structures, subcircuits shared between projects, and tests that run on
every change. Writing them in Rust also brings its ecosystem along: Cargo, crates, the type checker,
editors with language servers, and `cargo test` in CI.

SKiDL pioneered describing circuits for KiCad in code, in Python, and is a great project to learn
from. cuprite_wire takes the same idea to Rust, with these aims:

- **Richer types.** Parts are types generated from their libraries, pins can be named constants,
  and subcircuits declare their interfaces, so many mistakes are found by the compiler or by ERC
  with a precise source location.
- **SPICE dialects.** Vendor models are often written for PSpice or LTspice. cuprite_wire talks to
  ngspice directly and selects its compatibility mode from the models, so such models can be used
  without rewriting them.

cuprite_wire is one more option next to these tools, not a replacement for them. Choose whichever
fits your project and your team.

## Requirements

- Rust (edition 2024)
- KiCad 9: pcbnew to import the netlist, and its libraries if parts are generated from them
- ngspice (44 or later) for the simulations

The devcontainer in `.devcontainer/` has all of them.

## Documentation

| Document | Contents |
|---|---|
| [`doc/kicad_netlist_usage.md`](doc/kicad_netlist_usage.md) | describing circuits and exporting a KiCad netlist |
| [`doc/kicad_part_macro.md`](doc/kicad_part_macro.md) | generating parts from KiCad symbols and footprints |
| [`doc/spice_part_macro.md`](doc/spice_part_macro.md) | parts from SPICE models, dialects |
| [`doc/spice_netlist_usage.md`](doc/spice_netlist_usage.md) | test benches and simulation with ngspice |
| [`doc/erc.md`](doc/erc.md) | the electrical rules check |
| [`doc/kicad_netlist_format.md`](doc/kicad_netlist_format.md) | the KiCad netlist format |
| [`doc/spice_netlist_format.md`](doc/spice_netlist_format.md) | the SPICE netlist format |

Smaller examples: `examples/kicad_rc_filter.rs` and `examples/spice_rc_filter.rs`.

## License

MIT. See [`LICENSE`](LICENSE).
