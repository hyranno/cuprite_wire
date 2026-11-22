# Changelog

All notable changes to the crates of this workspace (`cuprite_wire`, `cuprite_wire_macros`,
`cuprite_wire_kicad_lib` and `cuprite_wire_embedded_hal`) are documented in this file. The crates
are released together with the same version.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crates
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (before 1.0, a minor version may
break the API).

## [Unreleased]

### Added

- `CircuitBuilder::erc_allow` (`ErcMark::Allow`): leaves the violations of one ERC rule on one
  net out of the report, like excluding a KiCad ERC marker.
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
- `Ngspice::new` and `SharedNgspice::new`, to give the program or the library (they can no
  longer be built with a struct literal).
- `PartPins` (`PartPins::of(&part)`): the pin IDs of a part and its internal connection groups,
  with overlapping groups merged. `Component::pins` holds those of each component.

### Changed

- `#[non_exhaustive]`, so that later versions can add fields and variants without breaking the
  API: `ErcMark`, the types the library builds for the user to read (`Netlist`, `Component`, `Net`,
  `NetNode`, `ErcReport`, `ErcViolation`, `NodeMark`, `ErcPin`, `Diagnostic`, `Note`, `NodeLabel`,
  `CosimPin`, `Port`), the enums `Waveform`, `ModelSource` and `SpicePartInfo`, and the types with
  constructors (`SpiceElement`, `IndependentSource`, `InputCharacteristics`,
  `OutputCharacteristics`, `Ngspice`, `SharedNgspice`), which can no longer be built with a struct
  literal. Their fields stay public.
- ERC follows KiCad 10: the only pin of a labeled net is reported as `LabelSinglePin` (a warning)
  instead of `PinNotConnected` (an error).
- `Net` has a new public field (`labeled`), and `Component` too (`pins`).
- `KicadPartInfo::check_pins` and `SpicePartInfo::check` take the pins as `&PartPins` (instead of
  the pin IDs, and the internal connections for `check`).
- `Ngspice::default()` runs `ngspice_con` on Windows (the console program of ngspice's Windows
  builds; `ngspice` there is the GUI).
- On Windows, paths in netlists and ngspice commands (`.include`, the `d_cosim` library, the
  output files of `SharedNgspice`) and in the `Sim.Library` field of generated parts are written
  with `/`.
- The supported platforms are documented (README): everything but building digital models for
  XSPICE `d_cosim` (gcc, Verilator) runs on Windows and macOS; those builds are Linux-only.
- `Verilator::build` reports a missing shim with `shim_dir` to set, and outside Linux a missing
  program or shim with the note that the build is supported on Linux only.

- Warnings of `to_netlist` are no longer printed to stderr. They are emitted with the `log` crate
  (`log::warn!`) and are still available from `Netlist::warnings` and the ERC report.
- The API documentation has `# Errors` and `# Panics` sections for the functions that return a
  `Result` or may panic.

### Fixed

- `Ngspice::run` always writes a `.spiceinit` into the run directory (`unset ngbehavior` without a
  compatibility mode), so the user's `~/.spiceinit`, which ngspice read otherwise, no longer
  affects the run. `SharedNgspice` still reads it; ngspice cannot prevent that on Linux and macOS.
- `kicad_part!` expands the text variables of the value from the part's fields as KiCad does, e.g.
  `${SIM.PARAMS}`, the value of `Simulation_SPICE:OPAMP`, which was written as is.
- ERC treated internal connection groups sharing a pin (e.g. `["4", "5"]` and `["4", "ep"]`) as
  separate stacks, so pins of the same type in them could be reported as conflicting.

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

[Unreleased]: https://github.com/hyranno/cuprite_wire/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/hyranno/cuprite_wire/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/hyranno/cuprite_wire/releases/tag/v0.1.0
