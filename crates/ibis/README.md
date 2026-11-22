# cuprite_wire_ibis

Reads IBIS (I/O Buffer Information Specification) model files (`.ibs`).

It is developed with [cuprite_wire](https://crates.io/crates/cuprite_wire), which will use it to
simulate I/O buffers with ngspice, but it does not depend on it.

The parser covers the header, `[Component]` (`[Manufacturer]`, `[Package]`, `[Pin]`) and the
`[Model]` keywords of the I-V behavior (`[Pullup]`, `[Pulldown]`, `[GND Clamp]`,
`[POWER Clamp]`, `[Ramp]`, the voltage ranges and references). Other keywords are skipped and
listed in `IbisFile::skipped`.

`spice::subcircuit` writes a model as a SPICE subcircuit for ngspice (with the XSPICE code
models). A driver has the ports `ku` and `kd`, which switch its pullup and pulldown from outside,
so that a test bench can play any sequence of states on it.

```rust
let ibs = cuprite_wire_ibis::parse(&std::fs::read_to_string("buffer.ibs")?)?;
let model = ibs.model("Output").unwrap();
```

## License

MIT
