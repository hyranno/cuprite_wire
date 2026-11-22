# Example: Sensor Front End

A single-supply (+5 V) analog front end that amplifies and filters a sensor signal for an ADC,
written the way a board is meant to be written with cuprite_wire:

- the board is made of **subcircuits**, each in its own module;
- some parts are **generated from library files** with `kicad_part!`;
- each subcircuit has **tests**: ERC on the subcircuit alone, and SPICE simulations in a test bench
  with simulation-only sources;
- `main` checks the **whole board** with ERC and writes its **KiCad netlist** for pcbnew.

```text
J1 IN ──> [preamp: AC coupling, gain 11] ──> [filter: 2nd-order low-pass, 1.1 kHz] ──> J1 OUT
                     ▲                                  ▲
                     └──────────── [bias: VREF = +5V / 2]
```

## Running

```sh
cargo run --example sensor_frontend > sensor_frontend.net   # ERC, then the netlist
cargo test --example sensor_frontend                          # the tests (needs ngspice)
```

`cargo test` runs these tests too (`test = true` in `Cargo.toml`). If ERC finds an error, `main`
prints it with the place in the source and writes no netlist, e.g. without the power flag of +5V:

```text
error: net "+5V": U1.vcc (power_in) is not driven by any power_out pin (add a `power_flag` if it is powered from outside)
  at examples/sensor_frontend/bias.rs:40:21
  in subcircuit instantiated at examples/sensor_frontend/main.rs:45:22
```

To import the netlist into pcbnew, add these footprint libraries to the footprint library table:
`cuprite_basic` (`libs/footprints/cuprite_basic.pretty`) and `sensor_frontend`
(`examples/sensor_frontend/libs/footprints/sensor_frontend.pretty`). The check scripts in `tools/`
find both: `python3 tools/check_netlist_import.py sensor_frontend.net`.

## Files

| File | Contents |
|---|---|
| `main.rs` | the board: connector J1, the subcircuits, power flags; ERC and the netlist |
| `parts.rs` | parts generated with `kicad_part!` from `libs/` |
| `bias.rs` | `MidSupply`: divider, filter and follower giving VREF |
| `preamp.rs` | `Preamp`: AC-coupled non-inverting amplifier biased at VREF |
| `filter.rs` | `LowPass`: unity-gain Sallen-Key low-pass filter around VREF |
| `bench.rs` | the test bench shared by the tests (supply, sources, ngspice) |
| `libs/symbols/sensor_frontend.kicad_sym` | symbols `OPAMP_RRIO` and `CONN_01x04` |
| `libs/footprints/sensor_frontend.pretty/` | the footprint of the pin header |
| `libs/models/rrio_opamp.lib` | the SPICE model of the op-amp |

The libraries are written for this example and released under the license of cuprite_wire (MIT).

## Parts from files

`parts.rs` reads the symbols, footprints and model at compile time:

- `RrioOpAmp`: the symbol has no `Sim.*` fields, as vendor symbols often do, so a `sim` block maps
  each symbol pin to a port of the `.subckt` by name. Its footprint is the SOT-23-5 of the basic
  library (`footprint_dir: "libs/footprints"`). The model clips the output at the supply rails.
- `Header1x04`: excluded from simulation by its symbol (`exclude_from_sim`); its footprint is in the
  example's own footprint library.

The other parts (resistors, capacitors) are the ready-made ones of `cuprite_wire::basic`.

## Subcircuits and their interfaces

Each subcircuit has an interface struct deriving `Interface` (`PreampPorts`, ...). Instantiating
the subcircuit returns its nodes in the board by name, and the ERC test gives the same fields their
pin types:

```rust
let preamp = board.instantiate_subcircuit(Named("preamp", Preamp::default()));
board.connect(preamp.vref, bias.vref);
```

The supply (`vcc`, `gnd`) is part of each interface, so a test can drive it and ERC can see where
it comes from. The board names the nets (`+5V`, `GND`, `IN`, `VREF`, `AMP`, `OUT`); the names of
nets inside a subcircuit are prefixed with its instance name (`/preamp/...`).

## Tests

| Subcircuit | ERC (`check_subcircuit`) | Simulation |
|---|---|---|
| `bias` | supply ports `power_out`, VREF read as `input` | power-up transient: VREF settles at half of 3.3 V and 5 V, under a 1 mA load |
| `preamp` | input and VREF driven (`output`) | AC: gain 20.8 dB and high-pass corner 1.6 Hz; transient: 110 mV out for 10 mV in, clipping at the rails |
| `filter` | input and VREF driven (`output`) | AC: 0 dB pass band, -3 dB at 1.12 kHz, < -38 dB at 10 kHz; step response: final value and 5.7 % overshoot |
| board (`main.rs`) | the whole board, clean | (the connector is not simulated) |

The test benches use ideal sources (`IndependentSource`) for the supply, VREF and the input; they
cannot be exported to KiCad, which keeps them out of the board. Analyses that only look at a
small-signal or DC solution do not notice an op-amp wired with positive feedback (the operating
point finds the unstable balance, and the AC gain has almost the same magnitude), so each
op-amp is also checked with a transient analysis.
