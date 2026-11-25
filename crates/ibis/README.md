# cuprite_wire_ibis

Reads IBIS (I/O Buffer Information Specification) model files (`.ibs`).

It is developed with [cuprite_wire](https://crates.io/crates/cuprite_wire), which uses it to
simulate I/O buffers with ngspice (feature `ibis`), but it does not depend on it.

`parse` reads the text of a file into an `IbisFile`: the header, the components with their
package and pins, and the models with their I-V tables and ramp rates. `parse_bytes` reads the
bytes of a file, replacing those that are not UTF-8 (in the comments of some vendor files).

```rust
let ibs = cuprite_wire_ibis::parse(
    "\
[IBIS Ver] 2.1
[File Name] buffer.ibs
[Component] Example
[Manufacturer] Nobody
[Package]
R_pkg 50m NA NA
L_pkg 2n NA NA
C_pkg 1p NA NA
[Pin] signal_name model_name
1 OUT buffer
2 GND GND
[Model] buffer
Model_type Output
C_comp 2p NA NA
[Voltage Range] 3.3 3.0 3.6
[Pulldown]
0 0 NA NA
3.3 50m NA NA
[Pullup]
0 0 NA NA
3.3 -50m NA NA
[Ramp]
dV/dt_r 2.0/1n NA NA
dV/dt_f 2.0/1n NA NA
[END]
",
)
.unwrap();
let model = ibs.model("buffer").unwrap();
assert_eq!(model.model_type, cuprite_wire_ibis::ModelType::Output);
assert_eq!(model.voltage_range.unwrap().typ, 3.3);
```

## Coverage

The parser follows the syntax of the IBIS specification (up to version 7): keywords are
case-insensitive and treat `_` and spaces alike, `[Comment Char]` changes the comment
character, numbers take scale suffixes (`p`, `n`, `m`, `k`, `M`, ...) and ignore the units
after them, and `NA` marks missing min/max values. It reads:

- the header: `[IBIS Ver]`, `[Comment Char]`, `[File Name]`, `[File Rev]`, `[Date]`,
  `[Source]`, `[Notes]`, `[Disclaimer]`, `[Copyright]`;
- `[Component]` with `[Manufacturer]`, `[Package]`, `[Pin]`, `[Diff Pin]` and
  `[Series Pin Mapping]`;
- `[Model Selector]`;
- `[Model]` with its subparameters (`Model_type`, `Polarity`, `Enable`, `Vinl`, `Vinh`,
  `Vmeas`, `Vref`, `Cref`, `Rref`, `C_comp`, or the `C_comp_*` parts), `[Model Spec]`
  (`Vinl` and `Vinh`), `[Temperature Range]`, `[Voltage Range]`, the references
  (`[Pullup Reference]`, ...), the I-V tables (`[Pullup]`, `[Pulldown]`, `[GND Clamp]`,
  `[POWER Clamp]`), the terminations (`[Rgnd]`, `[Rpower]`, `[Rac]`, `[Cac]`), `[Ramp]`,
  `[Rising Waveform]` and `[Falling Waveform]`, and the passive elements of a `Series` model
  (`[R Series]`, `[L Series]`, `[Rl Series]`, `[C Series]`, `[Lc Series]`, `[Rc Series]`).

Other keywords and subparameters (`[Package Model]`, `[Submodel]`, `[Series Current]`,
`[Receiver Thresholds]`, IBIS-AMI, ...) are skipped without an error and listed in
`IbisFile::skipped`. The parser is lenient about what does not change the meaning: it does
not check line lengths, keywords may be indented, and the keywords of text (`[Notes]`, ...)
may repeat.

## Simulation

`spice::subcircuit` writes a model as a SPICE subcircuit for ngspice (with the XSPICE code
models). A driver has the ports `ku` and `kd`, which switch its pullup and pulldown from outside,
so that a test bench can play any sequence of states on it. Its edges reproduce the waveforms of
the model (or follow its `[Ramp]`). `spice::series_subcircuit` writes a `Series` model.

How cuprite_wire uses it is described in its guide
[Simulating I/O buffers with IBIS models](https://github.com/hyranno/cuprite_wire/blob/main/doc/ibis.md).

## License

MIT
