# Changelog

All notable changes to the crates of this workspace (`cuprite_wire`, `cuprite_wire_macros`,
`cuprite_wire_kicad_lib`, `cuprite_wire_embedded_hal` and `cuprite_wire_ibis`) are documented in
this file. The crates are released together with the same version.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crates
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (before 1.0, a minor version may
break the API).

## [Unreleased]

### Added

- `cuprite_wire::parts::PartExt` overrides the value, footprint, fields and flags of a part
  instance without writing a new part: `part.dnp()`, `exclude_from_bom()`,
  `exclude_from_board()`, `with_value(..)`, `with_footprint(..)` and `with_field(..)` return an
  `Overridden` part (chainable), so parts from `kicad_part!` can be marked DNP for the BOM.
- `KicadNetlist::bom` makes a bill of materials (`cuprite_wire::kicad::KicadBom`) and writes it
  as CSV in the format of `kicad-cli sch export bom`: components excluded from the BOM are left
  out, equal components are grouped into one row with reference ranges (`R1-R5`), and DNP
  components get a row of their own. `BomOptions` chooses the columns (including part fields such
  as `MPN`), grouping and whether DNP components are listed.
- `cuprite_wire_ibis`: a new crate reading IBIS model files (`.ibs`): the header, components with
  their package and pins, and models with their I-V tables, ramp rates, voltage ranges and input
  thresholds. Other keywords are skipped and listed. It does not depend on `cuprite_wire`, which
  will use it to simulate I/O buffers. `cuprite_wire_ibis::spice::subcircuit` writes a model as
  an ngspice subcircuit with the structure of KiCad's IBIS conversion (package, `C_comp`, the I-V
  tables as XSPICE `pwl` models), but with ports `ku` and `kd` that switch the driver from
  outside; `Edges::weights` makes their waveforms from a sequence of high, low and
  high-impedance states and the edges of the model. The edges reproduce its `[Rising Waveform]`s
  and `[Falling Waveform]`s (the weights solved from two waveforms into different fixtures, or
  from one with `ku + kd = 1`, as KiCad does), or else are linear from its `[Ramp]`.
  `[Model Selector]`s are read (`IbisFile::pin_model` takes the default or a chosen model),
  `[Model Spec]` gives `Vinl`/`Vinh` by corner (`Input`, `I/O` and `I/O_open_*` models without
  them take the defaults of the specification, 0.8 V and 2.0 V), and `Terminator` models
  (`[Rgnd]`, `[Rpower]`, `[Rac]`, `[Cac]`) are written as receivers. ECL models are written as their single-ended
  counterparts, `[Diff Pin]` and `[Series Pin Mapping]` are read, and
  `cuprite_wire_ibis::spice::series_subcircuit` writes the passive elements of a `Series` model.
  Checked against 20 IBIS files of real devices.
- `cuprite_wire::spice::ibis` (feature `ibis`): `IbisDiffBuffer`, `IbisDiffDriver` and
  `IbisDiffReceiver` (pins `p`, `n`, `gnd`) simulate the differential pairs of `[Diff Pin]`, the
  inverting pin driven with the complement of the sequence after `tdelay`, with the series
  termination that `[Series Pin Mapping]` puts between the pins; `IbisSeries` is a `Series`
  model as a part.
- `ibis_part!` (feature `ibis`): names the pins of an IBIS file a test bench uses and generates a
  type with a function per pin returning its `IbisBuffer` (`IbisDiffBuffer` for a pair,
  `IbisSeries` for a series model) at a corner. The file is read at compile time to check the
  component, the pins, the models chosen from `[Model Selector]`s, the pairs and that every model
  can be simulated at every corner, with errors listing what the file has; only its path is
  written into the code, and it is read again at run time (`spice::ibis::read_file`).
- `kicad_part!` (feature `ibis`): symbols with KiCad 10's `Sim.Device IBIS` become parts holding
  the `IbisDriver`, `IbisReceiver`, `IbisDiffDriver` or `IbisDiffReceiver` of their pin, on the
  symbol's pins, checked at compile time as by `ibis_part!`. `new(corner)` / `new(corner,
  sequence)` read the file at run time; the driver plays the given sequence, and
  `kicad_sequence()` returns the symbol's DC or rectangular stimulus as one. KiCad's corners per
  parameter are not supported yet (a warning). The `ibis` block gives the model for KiCad's
  library symbols (`Simulation_SPICE:IBIS_DRIVER`, ...), which have none.
- `kicad_part!`: `Sim.Pins` may name the model pins by number (`1=1 2=2`), as KiCad reads it.
- `cuprite_wire_ibis`: `LookupError`, `IbisFile::find_component`, `IbisFile::buffer_model`,
  `IbisFile::series_model` and `Component::find_diff_pin` find what a simulation asks for, with
  the reason when it is missing.
- `spice::TransmissionLine`: a lossless transmission line (SPICE `T`, as KiCad's
  `Simulation_SPICE:TLINE`) for test benches, such as the traces between IBIS buffers. The guide
  `doc/ibis.md` and the example `examples/ibis_signal_integrity/` show the simulation of links
  between chips with IBIS models.
- XSPICE code models inside a `.subckt` are supported by `spice_part!` and `kicad_part!`: files
  named by code model parameters (`d_state`'s `state_file`, `filesource`'s `file`, ...) are
  resolved relative to the library, checked and tracked for rebuilds. `sim_model: inline` writes
  them as absolute paths; with `sim_model: include`, a relative path is a compile error, since
  ngspice would look for it in the directory it runs in.

## [0.3.0] - 2026-11-22

### Added

- `CircuitBuilder::erc_allow` (`ErcMark::Allow`): leaves the violations of one ERC rule on one
  net out of the report, like excluding a KiCad ERC marker. It also works on a net without part
  pins (`NoConnectDangling`, `VirtualPinDangling`, `LabelOnlyNet`).
- `ErcRule::PinToPin`: its severity caps those of the pin map (`Warning` reports pin conflicts as
  warnings at most, `Ignore` turns them off).
- The ERC rules of KiCad 10 `ErcRule::LabelSinglePin` (a labeled net with only one pin) and
  `ErcRule::GroundPinNotGround` (a power pin named like a ground, such as `GND`, not on a ground
  net while another power pin of the part is), both warnings by default.
- `Net::labeled`: whether the net name is given by a label.
- Parts from the KiCad 10 libraries: `kicad_part!` reads KiCad 10's stacked pin notation
  (`[2,13]`, `[1-4]`) as a pin per pad, connected inside the part, and looks up the library
  directories in `KICAD10_SYMBOL_DIR` / `KICAD10_FOOTPRINT_DIR` before the `KICAD9_` ones. The
  netlist stays in KiCad 9's format, which KiCad 10 reads.
- Duplicate pins of KiCad 10 (several pins with the same number but different names or types,
  e.g. `Diode:BAT54SDW`): `kicad_part!` makes each a pin (IDs `6`, `6#2`, ...) connected inside
  the part, and the KiCad netlist writes the pad once, with the first pin as KiCad does.
  A part may have several pins on one pad if they are connected inside the part.
- `Ngspice::new` and `SharedNgspice::new`, to give the program or the library.
- `PartPins` (`PartPins::of(&part)`): the pin IDs of a part and its internal connection groups,
  with overlapping groups merged. `Component::pins` holds those of each component.
- `Component::pin_types`: the ERC pin types of the pins of each component.

### Changed

- **Breaking:** `Netlist::pinless_marks` is replaced by `Netlist::pinless_nets`: each net without
  part pins (`PinlessNet`) with its label names and ERC marks, so that `erc_allow` applies per net.
- **Breaking:** `#[non_exhaustive]`, so that later versions can add fields and variants without
  breaking the API: `ErcMark`, the types the library builds for the user to read (`Netlist`,
  `Component`, `Net`, `NetNode`, `ErcReport`, `ErcViolation`, `NodeMark`, `ErcPin`, `Diagnostic`,
  `Note`, `NodeLabel`, `CosimPin`, `Port`), the enums `Waveform`, `ModelSource` and `SpicePartInfo`,
  and the types with constructors (`SpiceElement`, `IndependentSource`, `InputCharacteristics`,
  `OutputCharacteristics`, `Ngspice`, `SharedNgspice`), which can no longer be built with a struct
  literal. Their fields stay public.
- ERC follows KiCad 10: the only pin of a labeled net is reported as `LabelSinglePin` (a warning)
  instead of `PinNotConnected` (an error).
- **Breaking:** `KicadPartInfo::check_pins` and `SpicePartInfo::check` take the pins as `&PartPins`
  (instead of the pin IDs, and the internal connections for `check`).
- `Ngspice::default()` runs `ngspice_con` on Windows (the console program of ngspice's Windows
  builds; `ngspice` there is the GUI).
- On Windows, paths in netlists and ngspice commands (`.include`, the `d_cosim` library, the
  output files of `SharedNgspice`) and in the `Sim.Library` field of generated parts are written
  with `/`.
- The supported platforms are documented (README): everything but building digital models for
  XSPICE `d_cosim` (gcc, Verilator) runs on Windows and macOS; those builds are Linux-only.
- `Verilator::build` reports a missing shim with `shim_dir` to set, and outside Linux a missing
  program or shim with the note that the build is supported on Linux only.
- **Breaking:** the elements of the output stages in generated subcircuits are named after the pin,
  as those of `CosimDevice` were: `DigitalDriver` writes `VD_out`, `RO_out`, `VE_out`, `SO_out`
  (instead of `VD`, `RO`, `VE`, `SO`) with the nodes `out#d`, `out#e`, and the drive source of a
  `DCosim` output is `ED_<pin>` (instead of `EO_<pin>`). The names of the subcircuits change with
  them.
- **Breaking:** instantiating: `CircuitBuilder::instantiate_part` and `instantiate_subcircuit`
  replace the six `instantiate_*` methods. An explicit name is given by wrapping the part or the
  generator in `circuit::Named` (`Named("R_LOAD", part)`, instead of `instantiate_part_named` and
  `instantiate_subcircuit_named`), and a generator with its own options by
  `SubcircuitGenerator::with_options` (instead of `instantiate_subcircuit_with` and
  `instantiate_subcircuit_named_with`). `instantiate_part` takes an `impl Part` or a
  `Box<dyn Part>`, so `IndependentSource::voltage` / `current`, `DigitalDriver::new`,
  `DigitalLoad::new`, `DCosim::new`, `CosimDevice::new` and `VerilatedModel::part` return the part
  instead of a `Box`, and their builder methods take `self`.
- **Breaking:** nodes are `circuit::Node` instead of petgraph's `NodeIndex`, and the type
  parameter of `Interface` defaults to it. A node used in a builder that did not create it, such
  as a node of the circuit around a subcircuit used inside it, is reported as
  `CircuitErrorKind::ForeignNode` instead of silently connecting to another node (or panicking).
- **Breaking:** `Part::pin_types` is replaced by `Part::extra_pin_types`: ERC takes the pin types
  from the KiCad pin information and gives these only to pins without it, so the two can no longer
  disagree. A pin in both is a `CircuitErrorKind::DuplicatePinType`, and a key that is not a pin
  ID an `UnknownPinId`.
- **Breaking:** `Part::get_ref_base_name` is renamed `Part::ref_prefix`, and the `ref_base` key of
  `kicad_part!` and `spice_part!` `ref_prefix`.
- **Breaking:** `spice_number` (from `spice::sources`) and `spice_value` (from `spice::part`) are in
  the new module `spice::value`, and re-exported from `spice`.
- **Breaking:** `SimulationError::SharedLibrary` and `SimulationError::Cosimulation` exist only with
  the `shared-ngspice` feature. `VerilatorError::Command::dir` is an `Option`: `None` for
  `verilator --version`, which does not run in a build directory.
- `Verilator` caches the builds in `cuprite_wire/verilator` in the user's cache directory
  (`$XDG_CACHE_HOME`, else `~/.cache`; `%LOCALAPPDATA%` on Windows; the system temporary
  directory only if none of them is set) instead of `cuprite_wire-verilator` in the system
  temporary directory, where another user could place libraries that would be loaded. On Unix, the cache directory is created with mode `0700`, and
  `build` fails with `VerilatorError::Io` if it is writable by the group or others, or (on Linux)
  belongs to another user.
- Warnings of `to_netlist` are no longer printed to stderr. They are emitted with the `log` crate
  (`log::warn!`) and are still available from `Netlist::warnings` and the ERC report.
- The API documentation has `# Errors` and `# Panics` sections for the functions that return a
  `Result` or may panic.

### Removed

- **Breaking:** the public net graph (`NetGraph`, `NodeInfo`, `BuiltCircuit::nets`,
  `BuiltCircuit::connected_nodes`) and the `circuit::NodeIndex` re-export.
- **Breaking:** `SubcircuitInstance` is no longer public, and `kicad::PinType` is removed (use
  `parts::PinType`).

### Fixed

- `Ngspice::run` always writes a `.spiceinit` into the run directory (`unset ngbehavior` without a
  compatibility mode), so the user's `~/.spiceinit`, which ngspice read otherwise, no longer
  affects the run. `SharedNgspice` still reads it; ngspice cannot prevent that on Linux and macOS.
- `kicad_part!` expands the text variables of the value from the part's fields as KiCad does, e.g.
  `${SIM.PARAMS}`, the value of `Simulation_SPICE:OPAMP`, which was written as is.
- ERC treated internal connection groups sharing a pin (e.g. `["4", "5"]` and `["4", "ep"]`) as
  separate stacks, so pins of the same type in them could be reported as conflicting.
- A transient analysis that ngspice stops in the middle (`Timestep too small`, then
  `tran simulation(s) aborted`) is a `SimulationError::Ngspice`; ngspice prints no `Error` line
  for it and exits with 0, so the run succeeded with truncated results.
- `DigitalSequence::voltage_waveform` and `enable_waveform` repeated a time point for two events
  at one time that cancel each other (`.at(1.0, High).at(1.0, Low)`), which made the source panic.
- Reading SPICE libraries for generated parts: `;`, `//` and `$` inside quotes or `{...}` (e.g.
  `.include "a;b.lib"`) were taken as the start of a comment, and a value whose exponent
  overflows with its prefix (`1e2147483647P`) panicked.
- `KicadPartInfo::check_pins` reported a pad shared by three pins twice, and put pins that are not
  in the part on their pads, which could add a `DuplicatePad`. Problems are now in the order
  missing pins, unknown pins, pads.
- Reference prefixes (`Part::ref_prefix`) are checked when the part is instantiated: one
  that is empty, ends with a digit (`U1` + `1` and `U` + `11` were both `U11`), or contains
  whitespace, `?`, `#`, `/` or `{` is an `InvalidName`, and so is a `SubcircuitGenerator::base_name`
  that is empty or contains `#` or `{` when it makes an automatic name. The `ref_prefix` of
  `kicad_part!` and `spice_part!` must not end with a digit either.
- `kicad_part!` and `spice_part!`: pin IDs given in `pins` that differ only in case (`out` and
  `OUT`), which made two constants `OUT`, and a key given twice in a `{ ... }` map (`fields`,
  `params`, `pins`) are errors. `fields` cannot give `Sim.*` fields, which were written next to
  those of the `sim` block (or were not used for the simulation); give the model with `sim`.
- A `.model` with `pchan` only in an inline comment (`$ pchan`) was taken as a P-channel VDMOS,
  and words in inline comments were looked up as definitions the model needs.
- `${KIPRJMOD}` in `Sim.Library` is KiCad's project directory, the directory relative paths are
  resolved against (`sim_library_dir`), instead of an environment variable.
- `Simulation::table` reports a file that cannot be read (e.g. no command wrote it) as
  `SimulationError::Table` with the file name, instead of `Io` ("cannot run ngspice").
- `Verilator::build` read the sources as UTF-8 for the cache key, so a source with a Latin-1
  comment could not be built; it reports a build that cannot be moved into the cache instead of
  failing later; and the logs of ngspice and Verilator start the standard error on a new line.
- `spice_part!` reports a `sim` key (`suffix`, `unused`) as unknown before reading its value, and
  names the model when a port in `pins` is not found.
- Reading KiCad files: escapes in strings follow KiCad's lexer (`\a` `\b` `\f` `\v`, bytes in hex
  `\x41` and octal `\101`; a backslash before another character is kept), and a pin in several
  units with the same name and type belongs to the lowest of them rather than the first in the
  file.
- Co-simulation: a transient analysis with a `tstart` called the models first at that time, as the
  first point (`is_first_point()`), and `option interp` called them many times at the same time;
  both are now `SimulationError::Cosimulation`.
- `SharedNgspice`: `set` and `option` commands of a run carried over to the next runs in the
  process; the library is now loaded for each run and unloaded after it. The commands of the
  background thread (`bg_run`, ...), which made the run end while the analysis went on, are a
  `SimulationError::SharedLibrary`.
- `cuprite_wire_embedded_hal`: a PWM duty set by the firmware at the time a period starts took
  effect in that period instead of the next (`Firmware` now updates the PWM channels before the
  step; `Pwm::update` documents the order). An output pin that has not been set is neither
  `is_set_high` nor `is_set_low`, since a bidirectional pin starts in `HiZ`.

## [0.2.0] - 2026-11-21

### Added

- `cuprite_wire_embedded_hal`: a new crate implementing the `embedded-hal` 1.0 digital and PWM
  traits, and a minimal `AdcChannel` trait, on the pins of a `CosimDevice`, so that firmware
  written against `embedded-hal` runs against the simulated circuit (`Board`, `Firmware`).
- Co-simulation with Rust device models (`spice::cosim`, feature `shared-ngspice`): `CosimDevice`,
  the `DeviceModel` trait with `DeviceIo` (voltages, input levels and edges, buses, breakpoints,
  `PinIndex`), `SpiceDeck::run_with`, and the `Periodic`, `Adc` and `Pwm` peripherals.
- `SharedNgspice`: runs a deck in the ngspice shared library (feature `shared-ngspice`), loaded at
  run time. The CLI and the library backends implement the new `Simulator` trait.
- XSPICE `d_cosim` part (`spice::xspice::DCosim`) for digital models compiled into a shared library,
  and `xspice::Verilator` to build Verilog designs into such models (`VerilatedModel::part`).
- Digital stimuli for test benches (`spice::digital`): `DigitalSequence` with UART and PWM helpers,
  `OutputCharacteristics` / `InputCharacteristics`, the `DigitalDriver` and `DigitalLoad` parts,
  and `InputCharacteristics::decode` to read simulated voltages back as logic levels.
- Typed subcircuit interfaces: the `subcircuit::Interface` trait and `#[derive(Interface)]`.
- `Simulation::expect_value`, which panics with the output of ngspice when a value is missing.
- All public items are documented, and the documentation on docs.rs shows which items need a
  feature (such as `shared-ngspice`).
- Pin name constants `IndependentSource::P` / `N` and `Ammeter::P` / `N`.
- `circuit::NodeIndex` is re-exported, so naming it no longer needs a dependency on `petgraph`.
- KiCad value notation is converted to SPICE's as KiCad does (SI prefixes with `M` for mega, the
  RKM code such as `4k7`, and units such as `100nF`), in values, `Sim.Params` and the values given
  at run time to `value: param` parts.

### Changed

- The minimum supported Rust version (MSRV) is 1.89, declared as `rust-version` and checked in CI.
- **Breaking:** `SubcircuitGenerator` has an `Interface` type. The generator gets its interface
  nodes in `construct_subcircuit` and returns them; `instantiate_subcircuit*` returns the outer
  nodes in the same type instead of a map. `check_subcircuit` takes the pin types in the interface
  type holding `Option<PinType>`. `SubcircuitInstance::node_mapping` is renamed to `interface`.
- **Breaking:** `instantiate_subcircuit` and `instantiate_subcircuit_named` take only the generator,
  whose options are those of the circuit. A generator with its own options is instantiated with
  `instantiate_subcircuit_with` / `instantiate_subcircuit_named_with`.
- **Breaking:** `ErcRule`, `PinInfoProblem`, `SpiceProblem`, `SimulationError` and `VerilatorError`
  are `#[non_exhaustive]`; add a wildcard arm when matching them.
- **Breaking:** explicit instance names and net labels may not contain `{` (it starts the escape
  sequences of KiCad names, so `I/O` and `I{slash}O` became the same net).
- **Breaking:** NaN and infinite numbers in SPICE values, sources and input thresholds, and PWL
  waveforms without points or with times not increasing, panic instead of writing an invalid deck.
- `LibId` and the SPICE dialect enum moved to `cuprite_wire_kicad_lib` and are re-exported from
  `cuprite_wire`. `cuprite_wire_kicad_lib` now uses the stricter `LibId` validation of
  `cuprite_wire`.
- The code generated by `kicad_part!` and `spice_part!` names standard items by their full paths.
- Tests and examples are no longer included in the published packages.

### Removed

- **Breaking:** the `sexpr` module and `KicadNetlist::to_sexpr` (write the netlist with `Display`
  instead), `naming::escape_name`, `naming::unescape_name`, the `natural_cmp` re-export,
  `pin_refs_by_node`, `collect_connected_part_pins`, `parts::PinRef` and `ExpandedCircuit`.

### Fixed

- Subcircuits of `DigitalDriver` and `CosimDevice` named a port `gnd`, which ngspice reads as the
  ground, so a `gnd` pin not connected to the ground was ignored.
- Internal nodes of `DCosim` and `CosimDevice` pins could collide with pins named like them, and
  pins named `ref` or `0` were shorted to the reference or the ground; such names are now rejected.
- `SharedNgspice` runs the dot analyses of a deck without a control block, as `ngspice -b` does.
- Only lines starting with the word `error` are reported as ngspice errors (a `meas` result named
  `error_max` no longer fails the run).
- `write` without a file name writes into the run directory instead of the working directory.
- A second transient analysis in one co-simulation run starts the peripherals and the firmware over.
- A save leaving out the node of a device pin returns `SimulationError::Cosimulation` instead of
  reading 0 V.
- The Verilator build works with relative paths and rebuilds when an included file changes; ports
  whose pins are not valid pin names return `VerilatorError::Ports` instead of panicking.
- A library including more than 256 files returns an error instead of skipping the rest.
- The channel of VDMOS models is checked against `Sim.Device` `NMOS` / `PMOS`.

## [0.1.0] - 2026-11-19

Initial release: describing circuits in Rust, ERC, export to KiCad 9 (pcbnew) and SPICE netlists,
simulation with ngspice, and parts generated from KiCad libraries and SPICE models
(`kicad_part!`, `spice_part!`).

[Unreleased]: https://github.com/hyranno/cuprite_wire/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/hyranno/cuprite_wire/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/hyranno/cuprite_wire/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/hyranno/cuprite_wire/releases/tag/v0.1.0
