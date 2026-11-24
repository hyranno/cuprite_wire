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

## 2. Typed analyses and probes

`spice::analysis` writes `op`, `tran`, `dc` and `ac` with the vectors to record (`Probe`), and
reads their results without file or column names. The string commands of `SpiceDeck::control`
stay, and both go into the `.control` block in the order they are added, so `meas`, `let`,
`setplot` and analyses not covered by types can be mixed in.

- Probes are made by the deck (`deck.voltage("/OUT")`, `voltage_between`, `current("V1")`), as is
  the source a `Dc` sweeps (`deck.source("V1")`, a `V` or `I` element), not by
  the netlist or the analysis: the deck is the test bench, `deck.netlist()` knows the nets and
  elements to check the name against, and `SpiceDeck::run_with` of the co-simulation takes the same
  deck. An unknown net or source is an error (`ProbeError`) naming it, before the run, not an
  ngspice error after it.
- `SpiceDeck::analysis(&a)` appends the analysis command and a `wrdata analysis<n>.txt <probes>`
  right after it (`wrdata` writes the current plot). The deck records the analysis by an identity
  (`id`) with its file, and `Simulation` copies the record, so `sim.result(&a)` finds the file. The
  identity is a process-wide counter renewed by every builder method (`probe`, `probes`, the options of `Tran`), so an
  analysis that the deck did not run, including one of another deck or one changed after it was
  added, is an error (`SimulationError::Analysis`), not the result of another analysis. The file
  names count per deck, so the deck text is deterministic. Adding the same analysis twice panics.
- Results are read by position, not by the column names: the generator knows the columns (the
  scale, then each probe, two columns for complex values), and the names ngspice writes are not
  stable (`v("/OUT")` becomes `v(/OUT)`, the scale of `op` is a node of the plot; see
  [SPICE netlist format](spice_netlist_format.md) 6). The count of columns is checked.
- AC values are written as `real(<probe>) imag(<probe>)` and returned as a small `Complex` with
  `norm`, `db`, `phase` (radians, as `vp()`) and `phase_degrees`, without a dependency on a complex
  number crate. `AcResult` computes `continuous_phase` (`cph()`) and `group_delay` from the values
  instead of recording `cph()` and `group_delay()` of ngspice: the probes stay the vectors, any
  probe (including a voltage between nodes) gets them, and the ends of the scale use a plain
  one-sided difference (see [SPICE netlist format](spice_netlist_format.md) 6).
- `AcResult` has no `interpolate`: between frequencies, linear interpolation of the real and
  imaginary parts on a linear frequency is not what a Bode plot shows (dB and phase on a log
  frequency), and no choice fits all uses. A value at a frequency is asked for with the scale
  (`Ac::linear`, or a frequency on the `Ac::decade` grid), and a crossing with `meas`.
- `op` is read from `wrdata` as the other analyses, not by parsing `print` in the log.
- The result types are `OpResult` (`get -> f64`), `SweepResult` for `tran` and `dc` (`scale`,
  `get -> &[f64]`, `interpolate`) and `AcResult` (`frequency`, `get -> &[Complex]`). Asking for a
  probe the analysis does not record is a `SimulationError::Analysis`, so tests can use `?`.
- `Tran` writes `tran <step> <stop> [<start> [<max step>]] [uic]`: a maximum step without a start
  writes a start of 0, since the arguments are positional. A `Dc` sweeps one source; a nested
  sweep (`dc V1 ... V2 ...`) would need a two-dimensional result and is left out until needed.
- The `Analysis` trait is sealed: the identity, the command and the reading of the columns are
  internal, and new analyses are added in the crate.
- `Probe::expression()` gives the ngspice expression (`v("/OUT")`, `v("/IN")-v("/OUT")`, `i(V1)`)
  for string commands. `meas ... WHEN` does not take expressions, only vectors and `vdb(<node>)`.
- The examples and the tests recording a line over time (`LineDeck` in `tests/common/mod.rs`,
  shared by the unit tests through `src/test_parts.rs`) use the typed analyses. Some tests keep
  `wrdata` and `Simulation::table` on purpose: those of `Table` itself (column names, quoting,
  the same table from both backends, the paths the shared library rewrites, decks that ngspice
  rejects), those comparing with vectors of ngspice (`vp()`, `cph()`, `group_delay()`), and
  those measuring with `meas` (`WHEN`, `FIND ... AT`), which needs the node name in `vdb(...)`.
  The tests of XSpice models, vendor IBIS files and `cuprite_wire_embedded_hal` still write
  `wrdata`; they work, and are rewritten when they are changed for another reason.
