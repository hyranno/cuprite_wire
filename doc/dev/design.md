# Design Decisions

Decisions about the API that a maintainer needs when adding to it, with their reasons.

## 1. Part types and instance overrides

What a part *is* belongs to its type: the pins and their IDs, the internal connections, the pads
of the footprint, and the model derived from the symbol. `kicad_part!` and `spice_part!` check
them against the libraries at compile time, and the generated pin ID constants depend on them.
Options of the macros are only for these.

What is *added to or replaced in* the information of a part without changing the circuit
belongs to `PartExt` (`src/parts/overrides.rs`), for every kind of part (hand-written, `kicad_part!`,
`spice_part!`): the value, the footprint, fields, the DNP / BOM / board flags, and the SPICE
description (`with_spice`). New overrides of this kind go there, not into macro options, so that
there is one place to look for them.

- The pins are not overridable: a different pin set is a different part, and the pin ID
  constants and the checks against the symbol and footprint would no longer hold.
- `with_spice` is preferred to a `spice: custom` option of `kicad_part!` calling a trait the user
  implements for the generated type: the override works for every kind of part and also replaces
  a model the macro did derive, at the cost of being given per instance (a function returning the
  overridden part makes it reusable) and of a missing description being found when the SPICE
  netlist is made rather than at compile time.
- The KiCad overrides (`with_value`, ...) do not change the SPICE description, which is replaced
  only by `with_spice`, so that what is simulated never changes implicitly.
