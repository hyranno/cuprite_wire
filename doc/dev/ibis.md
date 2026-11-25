# IBIS Models and KiCad's IBIS Conversion

This document is for maintainers of the IBIS support (`crates/ibis`, `cuprite_wire::spice::ibis`,
`ibis_part!` and the IBIS symbols of `kicad_part!`). It records how KiCad converts IBIS models,
how to reproduce the conversion, and why cuprite_wire has its own implementation shaped like
KiCad's. For using the IBIS support, see `doc/ibis.md`; for the tests that compare the two, see
`doc/dev/verifying.md` section 1.

> Sources:
>
> 1. Actual runs of `kicad-cli sch export netlist --format spice` of KiCad 10.0 on copies of the
>    demo `demos/simulation/ibis` (last checked with 10.0.8)
> 2. Actual runs of ngspice 44.2 on the generated subcircuits
> 3. The IBIS specification (version 7)
>
> See "References" at the end for details.

## 1. KiCad's IBIS conversion (KIBIS)

KiCad converts IBIS models into SPICE subcircuits with KIBIS, C++ code inside eeschema. It has no
API and no command of its own: `kicad-cli sch export netlist --format spice` writes the
subcircuit of each IBIS symbol to its cache as a side effect of exporting the netlist, and the
netlist includes it.

- The cache is `$XDG_CACHE_HOME/kicad/<major>.<minor>/ibis/<ref>.cache` (`~/.cache/...` by
  default), one file per symbol, included by an absolute `.include` line.
- The symbol becomes `X<ref> <nodes> <ref>.<component>`, such as `XU4 GND RECT_OUTPUT U4.Virtual`.
- The subcircuit of a single-ended pin has the ports `GND` and `PIN`.
  Inside are the package (`R_pin`, `L_pin`, `C_pin`), `C_comp`, the I-V tables as XSPICE `pwl`
  models with `input_domain=0.05 fraction=TRUE`, and ideal sources for the supplies.
- A driver weights its pullup and pulldown by two PWL voltage sources `Vku` and `Vkd` inside the
  subcircuit. Their waveforms come from the stimulus of the symbol (`Sim.Params`), so the stimulus is
  part of the model.
- The subcircuit ends with `.ENDS DRIVER`, whatever its name; ngspice accepts this.

### 1.1 Symbol fields

| Field | Values |
|---|---|
| `Sim.Device` | `IBIS` |
| `Sim.Type` | `DEVICE` (receiver), `DCDRIVER`, `RECTDRIVER`, `PRBSDRIVER` |
| `Sim.Library` | the `.ibs` file (the demo gives it relative to the project) |
| `Sim.Name` | the `[Component]` |
| `Sim.Ibis.Pin` | the pin of the `[Pin]` table |
| `Sim.Ibis.Model` | the `[Model]` |
| `Sim.Params` | the stimulus: `dc` (`DCDRIVER`, such as `dc=high`), `ton toff td n` (`RECTDRIVER`), `f0 n` (`PRBSDRIVER`); the corners `vcc rpin lpin cpin` |

- KiCad 10 accepts only the `Sim.Type`s above. The IBIS symbols of its library
  (`Simulation_SPICE:IBIS_DRIVER`, `IBIS_DEVICE`, ...) still have the older types `IBISDRIVERDC`
  and `IBISDEVICE`, which KiCad 10 does not recognize: the export writes an empty `.cache`.
  KiCad replaces them once the symbol is edited in a schematic.
- A parameter of `Sim.Params` that the type does not know fails the export
  (`Unknown simulation model parameter 'foo'`).

### 1.2 Reproducing the conversion

The cache directory of the user may be read-only (it is in the Claude Code sandbox), so give the
export its own:

```sh
cp /usr/share/kicad/demos/simulation/ibis/* "$WORK"/
cd "$WORK"
XDG_CACHE_HOME="$WORK/cache" kicad-cli sch export netlist --format spice -o ibis.cir ibis.kicad_sch
ls cache/kicad/*/ibis/   # U1.cache ... U6.cache
```

To see how a field changes the conversion, edit the `Sim.*` properties of a symbol in the copy of
`ibis.kicad_sch` and compare its `X<ref>` line and its `.cache`. The tests do the same with the
`RECTDRIVER` U4 (`kibis_rectdriver` in `crates/ibis/tests/spice.rs`), replacing the demo's `.ibs`.

## 2. Why cuprite_wire has its own implementation

- **KiCad's output cannot be used directly.** The stimulus is baked into the subcircuit, so a
  bench cannot drive it with a `DigitalSequence`, and there is no API to run the conversion other
  than exporting a whole schematic.
- **KIBIS cannot be ported.** It is GPL-3.0, and this repository is MIT
  (`CONTRIBUTING.md`, "Licenses").
- **There was no Rust IBIS parser** (the crate `ibis` 0.1.0 on crates.io is an empty
  reservation). The Python parser eda-ibis-parser (MIT) was read for reference only.

The generated subcircuits keep the structure of KIBIS (section 1) so that the results can be
compared with KiCad's in the tests.

## 3. Design notes

- **The ports `ku` and `kd`.** Unlike KIBIS, the weights of the pullup and pulldown are ports of the
  subcircuit, driven from outside (`IbisDriver` puts the PWL sources in its own subcircuit). One
  model serves any sequence and is not regenerated for each.
- **The port `ref`.** The reference port is not named `gnd`, which ngspice reads as the global
  ground even as a port (`doc/dev/spice_netlist_format.md` section 6.2). KIBIS's `GND` is the global
  ground too, which is harmless there because the instance connects it to the ground anyway.
- **The weights reproduce the rounding of `pwl`.** `Edges::weights` solves the weights from the
  `[Rising Waveform]` / `[Falling Waveform]` with the I-V tables as `pwl` evaluates them, quadratic
  within `input_domain` (`PWL_DOMAIN`, 0.05 of the shorter adjacent segment) around each row. With
  the plain linear interpolation, the simulated edge missed a hand-made waveform by about 2.5 %.
- **`C_fixture`.** The current into the fixture is that of `R_fixture` to `V_fixture` plus
  `C_fixture * dV/dt`. Waveforms with `L_fixture`, `R_dut`, `L_dut` or `C_dut` other than 0 are not
  used and listed in `notes()`.
- **Default `Vinl` / `Vinh`.** The specification (checked in versions 4.2, 5.0 and 8.0, the table
  of the `[Model]` types) requires `Vinl` and `Vinh` of `Input`, `I/O` and `I/O_open_*` models and
  assumes 0.8 V and 2.0 V, with a warning of the parser, when they are missing.
  `Model::thresholds` falls back to them (`ModelType::default_thresholds`) and `IbisBuffer::notes()`
  lists each default used. `[Model Spec]` `Vinl` / `Vinh` override those of `[Model]`, so they take
  precedence over the defaults too. `Input_ECL` and `I/O_ECL` get no default: versions 4.2 and 5.0
  give -1.475 V and -1.165 V, while 8.0 repeats 0.8 V and 2.0 V, which looks like a copying error.
  Differential types get none either (`vdiff` of `[Diff Pin]` replaces the thresholds).
- **Upper-case scale suffixes are units.** The suffixes are case-sensitive (`T G M k m u n p f`),
  and other trailing letters are units, so `1.0PF` reads as 1.0 (farad). KIBIS does the same
  (`IbisParser::parseDouble` in `eeschema/sim/kibis/ibis_parser.cpp` of KiCad 10, read for reference
  only: an unknown letter is ignored because files write `3.0V`), and the specification lists only
  those suffixes. The parser follows KIBIS and does not warn: a warning would need a channel for
  every number of the file, and `IbisFile::skipped` is for keywords that are not read.
- **Nearly dependent waveforms.** Where the equations of the weights are dependent up to rounding
  (relative determinant below `1e-9`), the weights keep their previous values, as where there is no
  solution: the rounding error, clamped to 0..=1, made a weight jump between 0 and 1.
- **XSPICE file references are absolute.** A file parameter of a code model in a copied `.subckt`
  (`sim_model: inline`) is made absolute, because ngspice opens it relative to its working directory (`doc/dev/spice_netlist_format.md`
  section 5).

## References

| # | Source | Used for | License |
|---|---|---|---|
| 1 | Actual output of KiCad 10.0 `kicad-cli sch export netlist --format spice` on copies of `demos/simulation/ibis` | Section 1 | The output itself is not reproduced. |
| 2 | ngspice 44.2, runs of decks written for this project | Section 3 | The tests and results are this project's own. |
| 3 | IBIS Specification (up to version 8.0), IBIS Open Forum <https://ibis.org/> | Keywords, the weights of the waveforms, the default thresholds | IBIS Open Forum; text is not reproduced |
| 4 | eda-ibis-parser (Python) | Read for reference only | MIT |

### License notes

- **Do not copy or port KIBIS** (KiCad, GPL-3.0). Reading it to understand the behavior is fine.
- Do not commit KiCad's conversions of the demos or the demo's `.ibs`; the tests generate them at
  run time from the installed KiCad (ignored tests).
- IBIS files of vendors usually restrict redistribution: keep them out of the repository
  (`tests/vendor_ibis.rs` reads them from `CUPRITE_VENDOR_MODELS_DIR`). The `.ibs` files committed
  under `crates/ibis/tests/fixtures/` and `examples/ibis_signal_integrity/` were written for this
  project.
