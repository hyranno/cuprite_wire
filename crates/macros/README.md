# cuprite_wire_macros

Procedural macros of [cuprite_wire](https://crates.io/crates/cuprite_wire): `kicad_part!`,
`spice_part!`, `ibis_part!` and `#[derive(Interface)]`.

Do not depend on this crate directly. Use the macros through `cuprite_wire`
(`cuprite_wire::kicad_part!`, `cuprite_wire::spice_part!`, `cuprite_wire::ibis_part!` with the
feature `ibis`, `#[derive(cuprite_wire::subcircuit::Interface)]`), which documents them.

## License

MIT
