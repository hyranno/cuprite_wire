# Parts from IBIS Models

`ibis_part!` (feature `ibis`) names the pins of an IBIS file (`.ibs`) that a test bench uses. It
generates a type with one function per pin, returning the buffer of
`cuprite_wire::spice::ibis` for a corner, as `IbisBuffer::of_pin` and its siblings do at run time.
The file is read at compile time to check what is asked for, so a wrong pin, model or pair is a
compile error pointing at it instead of an `Err` when the test runs.

```rust
use cuprite_wire::{
    ibis::Corner,
    ibis_part,
    spice::{digital::*, ibis::*},
};

ibis_part! {
    /// The I/O of the microcontroller.
    pub struct Mcu {
        library: "${VENDOR_MODELS}/mcu.ibs",  // relative to the crate, absolute, or with ${VAR}
        component: "MCU",                     // optional when the file has one [Component]
        pins: {
            tx => "12",                                 // the pin's model
            io => { pin: "14", model: "IO_8MA" },       // a model of its [Model Selector]
        },
        diff_pins: { usb => "20" },                     // either pin of a [Diff Pin] row
        series: { odt => ["30", "31"] },                // a [Series Pin Mapping] row
    }
}

let tx = Mcu::tx(Corner::Typ)?;                         // IbisBuffer
let driver = bench.instantiate_part(IbisDriver::new(tx, seq));
let usb = bench.instantiate_part(IbisDiffReceiver::new(Mcu::usb(Corner::Typ)?));
```

`ibis_part!` and `spice_part!` work the same way: the library is located the same way (relative
to the crate, absolute, or with `${VAR}`), files read are tracked for rebuilds, errors point at
the option concerned, and only the path of the file is written into the code. The difference is
who reads the file at run time: ngspice reads a SPICE library itself, while an IBIS file is read
by `cuprite_wire`, which writes the subcircuits (`doc/spice_netlist_usage.md`, "IBIS buffers").

For a KiCad symbol simulating an IBIS pin (`Sim.Device IBIS`, or KiCad's
`Simulation_SPICE:IBIS_DRIVER` with the model given in the macro), `kicad_part!` generates a part
with the symbol's pins instead (`doc/kicad_part_macro.md` section 4, "IBIS models"); it shares the
checks of `ibis_part!`.

## 1. Options

| Key | Value | Default |
|---|---|---|
| `library` | the IBIS file: relative to the crate, absolute, or with `${VAR}` (environment variables) | required |
| `component` | the `[Component]` | the only one of the file |
| `pins` | `{ name => "pin", name => { pin: "pin", model: "model" } }`: buffers of single pins | none |
| `diff_pins` | the same, for differential pairs (`pin` is either pin of a `[Diff Pin]` row) | none |
| `series` | `{ name => ["pin1", "pin2"] }`: the `Series` model between two pins | none |

- Pins are the names of the `[Pin]` table (usually numbers such as `"12"` or `"A5"`), not the signal
  names.
- `model` chooses a model of the `[Model Selector]` the pin names (of both pins of a pair).
  Without it, the default (first) model is used, as `of_pin` does.
- Each `name` becomes a function, so names are unique across `pins`, `diff_pins` and `series`;
  `file` is taken.

## 2. The generated type

A unit struct (`Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, `Default`) with:

| Item | |
|---|---|
| `LIBRARY: &str` | the absolute path of the file |
| `COMPONENT: &str` | the `[Component]` |
| `file() -> Result<&'static IbisFile, IbisBufferError>` | the file, read and parsed at the first call (`spice::ibis::read_file`) |
| `name(corner) -> Result<IbisBuffer, IbisBufferError>` | for each entry of `pins` (`IbisBuffer::of_pin` or `of_pin_model`) |
| `name(corner) -> Result<IbisDiffBuffer, IbisBufferError>` | for each entry of `diff_pins` (`IbisDiffBuffer::of_pin` or `of_pin_model`) |
| `name(corner) -> Result<IbisSeries, IbisBufferError>` | for each entry of `series` (`IbisSeries::of_pins`) |

The type is not a part itself: put the buffers in `IbisDriver`, `IbisReceiver`, `IbisDiffDriver`
or `IbisDiffReceiver`, which take the stimulus (`DigitalSequence`) and the corner at run time. The
documentation of the type lists its functions with their pins, signals and models, so they can be
checked in the editor's hover.

The functions return an error only if the file cannot be read when the test runs (it moved, or
the program runs on another machine), or if it changed so that it no longer has what was checked.
A changed file is normally caught at compile time, since it triggers a rebuild.

## 3. Checks and errors

Each entry is checked at compile time as the run-time functions would check it, and every model
used is written as a subcircuit at the three corners, so the functions do not fail on a file that
has not changed. Errors point at the option concerned:

| Error | Points at | The message lists |
|---|---|---|
| the file is missing or not valid IBIS | `library` | the path, or the line that failed to parse |
| no such component, or several without `component` | `component` (or the type name) | the components of the file, similar names |
| no such pin | the pin | the pin with that signal name, or similar pin names |
| the pin is `POWER`, `GND` or `NC` | the pin | |
| `model` is not a model of the pin's selector | `model` | the models of the selector with their descriptions |
| the pin is in no `[Diff Pin]` row | the pin | the pairs of the component |
| no `[Series Pin Mapping]` row joins the pins | the pin | the mappings of the component |
| a model cannot be simulated at a corner | the pin | the model, the corner and the reason (an unsupported type, a missing table) |

```
error: component `MCU` has no pin `TX`; pins are named by number: the signal `TX` is on the pin `12`
error: pin `14` has no model: `IO_4MA` is not a model of `io_sel`; the models of [Model Selector] `io_sel`:
         IO_8MA  (8 mA drive)
         IO_2MA  (2 mA drive)
```

## 4. Licenses of vendor models

IBIS files of vendors usually restrict redistribution like their SPICE models
(`doc/spice_part_macro.md` section 7). `ibis_part!` writes only the path into the code, and
nothing of the file is copied: give it with an environment variable
(`library: "${VENDOR_MODELS}/mcu.ibs"`) to keep vendor files out of the repository.
