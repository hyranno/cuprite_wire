# Simulating I/O Buffers with IBIS Models

IBIS (I/O Buffer Information Specification) models describe the I/O buffers of a device by their
behavior: the I-V curves of the driver and of the clamp diodes, the edges into test loads, the
input capacitance and the parasitics of the package. Vendors publish them for most digital
devices. With the feature `ibis`, `cuprite_wire::spice::ibis` puts these buffers into a test bench,
drives them with a `DigitalSequence` and reads the levels back, so that the signal integrity of a
link (reflections, ringing, overshoot cut by the clamp diodes, edges into a real load) is tested
with `cargo test` like the rest of the board.

The files are read by the crate `cuprite_wire_ibis`, re-exported as `cuprite_wire::ibis`
(section 8). See `doc/spice_netlist_usage.md` for test benches and running ngspice; its section 3
compares IBIS buffers with the linear pin models of `cuprite_wire::spice::digital`, which are
simpler when only the logic levels matter. A complete example is
`examples/ibis_signal_integrity/` (section 7):

```sh
cargo run --example ibis_signal_integrity --features ibis
```

## 1. Overview

```rust
use cuprite_wire::{
    ibis::Corner,
    ibis_part,
    spice::{TransmissionLine, digital::*, ibis::*},
};

ibis_part! {
    pub struct Mcu {
        library: "${VENDOR_MODELS}/mcu.ibs",
        pins: { tx => "12", rx => "13" },
    }
}

let rx = Mcu::rx(Corner::Typ)?;
let seq = DigitalSequence::new(Level::High).uart(1e-6, 1e6, b"Hi");
let driver = bench.instantiate_part(IbisDriver::new(Mcu::tx(Corner::Typ)?, seq));
let trace = bench.instantiate_part(TransmissionLine::new(50.0, 1e-9));
let receiver = bench.instantiate_part(IbisReceiver::new(rx.clone()));
// ... connect driver[IbisDriver::OUT] - trace - receiver[IbisReceiver::IN], the grounds,
//     label the receiver's pin `RX`, run a transient analysis ...
let read = rx.input().unwrap().decode(table.scale(), table.column(r#"v("/RX")"#).unwrap());
```

- The feature `ibis` of `cuprite_wire` enables `spice::ibis`, `ibis_part!` and the IBIS symbols
  of `kicad_part!`.
- The I-V tables are simulated by the XSPICE code model `pwl` (`analog.cm`), which the usual
  builds of ngspice load.
- IBIS files of vendors usually restrict redistribution: keep them out of the repository and give
  their location with an environment variable (`doc/ibis_part_macro.md` section 4).

## 2. Getting the buffers

A buffer (`IbisBuffer`) is a `[Model]` of the file with the package parasitics of a pin, at a
corner. There are three ways to get one:

| | Run-time API | `ibis_part!` | `kicad_part!` |
|---|---|---|---|
| Gives | `IbisBuffer`, `IbisDiffBuffer`, `IbisSeries` | the same, one function per pin | a part (driver or receiver) with the pins of a KiCad symbol |
| The pin is checked | when the test runs (`Err`) | at compile time | at compile time |
| Use for | files chosen at run time, tools | test benches | KiCad symbols with `Sim.Device IBIS` |
| Documented in | this document | `doc/ibis_part_macro.md` | `doc/kicad_part_macro.md` section 4 |

```rust
let ibs = read_file("mcu.ibs")?; // or cuprite_wire::ibis::parse(&text)
let tx = IbisBuffer::of_pin(&ibs, "MCU", "12", Corner::Typ)?;
let io = IbisBuffer::of_pin_model(&ibs, "MCU", "14", "IO_8MA", Corner::Typ)?;
```

- `IbisBuffer::of_pin(&file, component, pin, corner)` takes the `[Model]` of a pin of a
  `[Component]` with the parasitics of the pin (`R_pin` / `L_pin` / `C_pin`, or else `R_pkg` /
  `L_pkg` / `C_pkg`); `IbisBuffer::new(&model, &parasitics, corner)` takes any model. Pins are
  the names of the `[Pin]` table (usually numbers), not the signal names.
- For a pin naming a `[Model Selector]`, `of_pin` takes its default (first) model and
  `IbisBuffer::of_pin_model(&file, component, pin, model, corner)` one of the others
  (`file.model_selector(name)` lists them with their descriptions).
- The errors (`IbisBufferError`) are `Read` (the file), `Lookup` (no such component, pin, model
  or pair, with what the file has) and `Subcircuit` (the model cannot be simulated).

## 3. Corners and models

`Corner::Typ`, `Corner::Min` and `Corner::Max` select the column of every value of the model:
the I-V tables, the edges, `C_comp`, the voltage range and the package. A value that is `NA`
falls back to typical; in a table, a column takes the rows that have values, and the typical
column if it has none. By the IBIS convention, `Min` is the slow and weak corner (lowest supply)
and `Max` the fast and strong one; simulate the three when the margins matter, as the example
does.

The supported model types are:

| Model types | `IbisDriver` plays | `IbisReceiver` |
|---|---|---|
| `Output` | `Low`, `High` | the driver off |
| `I/O`, `3-state` | `Low`, `High`, `HiZ` | the driver off |
| `Open_drain`, `Open_sink`, `I/O_open_drain`, `I/O_open_sink` | `Low`, `HiZ` | the driver off |
| `Open_source`, `I/O_open_source` | `High`, `HiZ` | the driver off |
| `Input` | | yes |
| `Terminator` | | with its `[Rgnd]`, `[Rpower]` and `[Rac]` + `[Cac]` |
| the ECL versions (`Output_ECL`, `Input_ECL`, ...) | as above | as above |
| `Series` | `IbisSeries` (section 5) | |

- A receiver is the package, `C_comp` and the clamps of the model. ECL models are simulated like
  their counterparts, with `[Pulldown]` relative to the supply, as the specification defines.
- `Polarity` (`Inverting`) and `Enable` (`Active-Low`) of the model are not applied: the sequence
  is the level of the pin (the pad), not of the logic inside the device.
- Beyond the voltages of a table, its end segments are extended. A table that ends early keeps
  its last slope: a clamp table ending at 0 V with a slope conducts on the other side too, so
  give tables over the whole range the pin can reach, as vendors' files usually do.

## 4. Drivers and receivers

- `IbisDriver::new(buffer, sequence)` (pins `out`, `gnd`; `XDRV1`) plays a `DigitalSequence`.
  The model's `[Pullup]` and `[Pulldown]` are weighted by two PWL sources that follow the edges of
  the model (`buffer.edges()`): the weights that reproduce its `[Rising Waveform]` /
  `[Falling Waveform]` in their test fixtures, delay included, as in the IBIS specification and
  KiCad (two waveforms with different `V_fixture` give both weights; one gives them with
  `ku + kd = 1`). An edge without waveforms switches linearly over the time of `[Ramp]` (its
  20-80 % time divided by 0.6, as KiCad does). Waveforms with a fixture other than `R_fixture`,
  `V_fixture` and `C_fixture` (`L_fixture`, `R_dut`, `L_dut`, `C_dut` other than 0) are not used
  and listed in `buffer.notes()`. `HiZ` turns both weights off. A level the model cannot drive
  (`HiZ` on an `Output`, `High` on an open drain) panics.
- `IbisReceiver::new(buffer)` (pins `in`, `gnd`; `XRX1`) is the package, `C_comp` and the clamps
  (and the termination of a `Terminator`).
- `buffer.input()` gives `InputCharacteristics` with the model's `Vinl` / `Vinh` (those of
  `[Model Spec]` at the corner if given) as the thresholds, to `decode` the voltage of the pin.
  An `Input`, `I/O` or `I/O_open_*` model without them takes the defaults of the specification,
  0.8 V and 2.0 V, listed in `buffer.notes()`; for other types (ECL, differential) it is `None`. Decode the voltage at the receiver's pin, where the device
  sees it.
- Both are subcircuits named after a hash of their definition (`cuprite_ibis_model_<hash>` for the
  model, `cuprite_ibis_<hash>` for the driver with its weights). The model has the ports `ku` and
  `kd` that switch its pullup and pulldown, so the sequence is not baked into the model, unlike
  KiCad's IBIS drivers.

## 5. Differential pairs

```rust
let tx = IbisDiffBuffer::of_pin(&ibs, "PHY", "A5", Corner::Typ)?; // either pin of the pair
let rx = IbisDiffBuffer::of_pin(&ibs, "FPGA", "B7", Corner::Typ)?;
let driver = bench.instantiate_part(IbisDiffDriver::new(tx, seq));
let receiver = bench.instantiate_part(IbisDiffReceiver::new(rx.clone()));
// ... connect `p` to `p` and `n` to `n` (through a pair of traces), run a transient analysis ...
let vdiff = rx.vdiff().unwrap(); // high above +vdiff, low below -vdiff
```

- `IbisDiffBuffer::of_pin(&file, component, pin, corner)` takes the row of `[Diff Pin]` with the
  pin: the buffers of its non-inverting and inverting pins (`p()`, `n()`, each with its own model
  and parasitics, as `IbisBuffer::of_pin`), `vdiff()` and `tdelay()` at the corner (0 when `NA`).
  `of_pin_model` chooses the model of a `[Model Selector]` for both pins;
  `IbisDiffBuffer::new(p, n, vdiff, tdelay)` pairs any two buffers.
- `IbisDiffDriver::new(buffer, sequence)` (pins `p`, `n`, `gnd`; `XDRV1`) plays the sequence on
  `p` and its complement on `n`, `tdelay` later (`HiZ` releases both). A pin whose model cannot
  drive high (open drain, as in TMDS) or low (open source) is released instead.
- `IbisDiffReceiver::new(buffer)` (pins `p`, `n`, `gnd`; `XRX1`) puts the receiver of each buffer
  on its pin. `cuprite_wire` does not decode the difference: compare `v(p) - v(n)` with
  `vdiff()`, for example with an `InputCharacteristics` whose thresholds are `-vdiff` and `vdiff`
  (as `read_bits` in the example).
- A `Series` model that `[Series Pin Mapping]` puts between the two pins of the pair, such as an
  on-die 100 ohm termination, is part of both the driver and the receiver (`termination()`;
  `with_termination` adds one to a pair made with `new`). It connects the pins outside their
  packages. `IbisSeries::of_pins(&file, component, pin1, pin2, corner)` / `IbisSeries::new(&model,
  corner)` (pins `a`, `b`; `XSER1`) is such a model as a part of its own: `[R Series]`,
  `[L Series]` with `[Rl Series]`, and `[C Series]` with `[Lc Series]` and `[Rc Series]`, in
  parallel.

## 6. Traces and terminations

Between the buffers, the board is ordinary parts of the bench:

- `TransmissionLine::new(z0, td)` (`cuprite_wire::spice`, pins `p1`, `n1`, `p2`, `n2`; `T1`) is a
  lossless line of impedance `z0` and delay `td`, as KiCad's `Simulation_SPICE:TLINE`. Connect
  `n1` and `n2` to the ground. A trace on FR-4 delays a signal by roughly 6 to 7 ps per
  millimetre; a trace whose delay is much shorter than the edges acts as a node and needs no
  line.
- A differential pair is two lines, uncoupled (100 ohms between them for two 50 ohm lines).
  Coupled lines are not provided.
- Terminations are parts of the circuit under test, such as `basic::Resistor0603`: a series
  resistor at the driver, a resistor to the ground or the supply at the receiver.

`TransmissionLine` has no KiCad information, like the sources: it belongs in the bench, around
the circuit under test.

## 7. The example

`examples/ibis_signal_integrity/` links two chips of a made-up IBIS file (`demo_io.ibs`, written
for the example; vendors' files are larger but read the same way) by traces of 1 ns:

- a 50 MHz clock from a 3.3 V LVCMOS output (about 16 ohms) on a 50 ohm trace to an LVCMOS input,
  without and with a 33 ohm series resistor at the driver;
- a bit pattern at 500 Mbit/s from an LVDS-like output pair on two 50 ohm traces to an input pair
  terminated on the die by 100 ohms (`[Series Pin Mapping]`).

```
clock, Min, series resistor none: -0.72 V to +3.61 V, read as sent, 2.00 to 2.10 ns later
clock, Min, series resistor   33: -0.00 V to +3.00 V, read as sent, 2.16 to 2.25 ns later
clock, Typ, series resistor none: -0.77 V to +4.06 V, read as sent, 1.73 to 1.77 ns later
clock, Typ, series resistor   33: -0.09 V to +3.34 V, read as sent, 1.88 to 1.89 ns later
clock, Max, series resistor none: -0.83 V to +4.40 V, read as sent, 1.59 to 1.59 ns later
clock, Max, series resistor   33: -0.31 V to +3.80 V, read as sent, 1.69 to 1.77 ns later
bits, Min: read as sent, 1.33 to 1.33 ns later
bits, Typ: read as sent, 1.29 to 1.29 ns later
bits, Max: read as sent, 1.25 to 1.26 ns later
```

Without the resistor, the edge doubles at the open end of the trace and rings; the receiver's
clamp diodes cut it at 0.6 to 0.8 V beyond the rails. With the resistor, the waveform typically
stays within the rails, but the strong driver of the `Max` corner still overshoots. The tests
(`cargo test --example ibis_signal_integrity --features ibis`) check this and that every edge
arrives as sent: the whole path from a `DigitalSequence` through the IBIS buffers and the
traces to `decode`.

## 8. The IBIS reader (`cuprite_wire_ibis`)

`cuprite_wire_ibis` reads `.ibs` files and writes their models as ngspice subcircuits; it does not
depend on `cuprite_wire` and can be used alone. `parse(&text)` reads a file into an `IbisFile`:
the header, the components (package, pins, `[Diff Pin]`, `[Series Pin Mapping]`), the model selectors
and the models. Keywords it does not read are skipped and listed in `IbisFile::skipped`.
`cuprite_wire_ibis::spice::subcircuit` writes a model with the ports `pin ref` (receiver) or
`pin ref ku kd` (driver), and `Edges::weights` the waveforms of `ku` and `kd` for a sequence of
states. The API reference is on [docs.rs](https://docs.rs/cuprite_wire_ibis).

The subcircuits have the structure of KiCad's IBIS conversion (the package, `C_comp`, the I-V
tables as `pwl` models), and their results are compared with KiCad's in the tests
(`doc/dev/verifying.md`). `doc/dev/ibis.md` describes KiCad's conversion and the design.

## 9. Not supported

- Keywords of the models: `[Submodel]` (bus hold, dynamic clamps), `[Driver Schedule]`,
  `[Receiver Thresholds]`, `[External Model]` (so the `*_diff` model types), `Series_switch`
  models and `[Series Current]` / `[Series MOSFET]`.
- Power-aware simulation: `[ISSO PU]` / `[ISSO PD]`, `[Composite Current]`; the supplies of the
  buffers are ideal sources inside their subcircuits.
- Packages: `[Package Model]` (the matrices of `[Define Package Model]`) and EBD files; each pin
  has its own R, L and C.
- IBIS-AMI.
