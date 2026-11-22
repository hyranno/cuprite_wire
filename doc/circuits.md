# Describing Circuits

This document explains how to describe a circuit with cuprite_wire: parts, nodes, labels and
subcircuits, and how errors are reported. The same circuit is exported to KiCad
(`doc/kicad_netlist_usage.md`), simulated (`doc/spice_netlist_usage.md`) and checked (`doc/erc.md`).
The API reference is on [docs.rs](https://docs.rs/cuprite_wire).
`examples/sensor_frontend/` shows a whole board with subcircuits, parts from its own libraries, and tests.

## 1. Pipeline

```rust
let netlist = circuit             // CircuitBuilder
    .build(&options)              // BuiltCircuit: subcircuits expanded (never fails)
    .to_netlist()?;               // Netlist: format-independent, nets resolved and named
netlist.to_kicad()?;              // KicadNetlist (doc/kicad_netlist_usage.md)
netlist.to_spice()?;              // SpiceNetlist (doc/spice_netlist_usage.md)
netlist.erc(&ErcSettings::default());   // ErcReport (doc/erc.md)
```

- `build()` always succeeds. Problems found while describing the circuit (invalid names, ...) are
  kept and reported by `to_netlist()`.
- `to_netlist()`, `to_kicad()` and `to_spice()` return `Err(CircuitErrors)` with **all** errors
  found, not just the first.
- The output is deterministic: the same circuit always gives the same bytes.

## 2. Parts

Parts are usually generated from KiCad libraries with `kicad_part!` (`doc/kicad_part_macro.md`) or
from SPICE models with `spice_part!` (`doc/spice_part_macro.md`). `cuprite_wire::basic` has
ready-made basic parts. Parts can also be written by hand.

`Part` is format-independent. Format-specific information is returned as an `Option`:

```rust
impl Part for Passive {
    fn get_ref_base_name(&self) -> String { "R".into() }      // reference prefix
    fn pin_ids(&self) -> Vec<String> { vec!["1".into(), "2".into()] }
    fn value(&self) -> String { "10k".into() }
    fn kicad(&self) -> Option<KicadPartInfo> { ... }         // doc/kicad_netlist_usage.md section 2
    fn spice(&self) -> Option<SpicePartInfo> { ... }         // doc/spice_netlist_usage.md section 2
    // fn internal_connections(&self) -> Vec<Vec<String>>   (empty by default)
    // fn pin_types(&self) -> BTreeMap<String, PinType>     (doc/erc.md section 2)
}
```

- **One pin ID corresponds to one footprint pad**, as in KiCad symbols. A part with several pads
  of the same function (e.g. six GND pins) has one pin ID per pad. The exception is KiCad 10's duplicate pins
  (several pins with different names or types on one pad), which must be connected inside the part.
- Pins connected inside the part are declared with `internal_connections()`; they are connected
  when the part is instantiated (like stacked pins in a KiCad symbol). Groups sharing a pin are
  merged into one. `PartPins::of(&part)` gives the pin IDs and the merged groups, as the netlist
  sees them (`Component::pins`); the checks of the format-specific information take it.
- Parts supporting different formats may be mixed in one circuit (e.g. a simulation-only voltage
  source next to a connector that has no SPICE model). Only exporting such a circuit to the
  unsupported format is an error (`KicadUnsupported`, `SpiceUnsupported`). There is no option to
  skip unsupported parts; for SPICE, parts that should not be simulated are excluded explicitly
  (`SpicePartInfo::Excluded`).

## 3. Nodes, parts and subcircuits

```rust
let mut circuit = CircuitBuilder::new();
let node = circuit.add_node();
let r = circuit.instantiate_part(resistor);          // automatic name
let c = circuit.instantiate_part(Named("c_bypass", capacitor));
circuit.connect(node, r["1"]);
circuit.label(c["2"], NetLabel::global("GND"));
let amp = circuit.instantiate_subcircuit(Named("amp_left", Follower));
circuit.connect(amp.output, node);
```

Nodes are `circuit::NodeIndex` (petgraph's `NodeIndex`, re-exported so that circuits can be
written without depending on petgraph).

A subcircuit is a `SubcircuitGenerator`. Its `Interface` type lists the nodes the circuit around
it connects to, usually as a struct deriving `subcircuit::Interface`, generic over the type of its
fields. The generator gets the interface nodes inside the subcircuit, and `instantiate_subcircuit`
returns the corresponding new nodes in the outer circuit, in the same struct:

```rust
#[derive(Interface)]
pub struct FollowerPorts<T = NodeIndex> {
    pub input: T,
    pub output: T,
    pub vcc: T,
    pub gnd: T,
}

impl SubcircuitGenerator for Follower {
    type Options = ();
    type Interface = FollowerPorts;
    fn construct_subcircuit(&self, _options: &(), c: &mut CircuitBuilder, ports: FollowerPorts) {
        // `c` has only the interface nodes; add the parts and connect them to `ports.input`, ...
    }
}
```

`Interface` is also implemented for arrays (`type Interface = [NodeIndex; 2]`) and for `()`.
The same struct holding other values gives them per interface node, e.g. the pin types of
`check_subcircuit` (`doc/erc.md` section 5).

The generator is run when the circuit is built. `instantiate_subcircuit(generator)` gives it the
options the circuit is built with (its `Options` must be the same type, e.g. `()`);
`generator.with_options(f)` instead derives its own options from them with the function `f`:

```rust
let amp = circuit.instantiate_subcircuit(Amplifier.with_options(|board: &BoardOptions| board.amp.clone()));
```

`instantiate_part` and `instantiate_subcircuit` record the calling source location with `#[track_caller]`, which is used in error
messages (section 6).

## 4. Instance names

Every part and subcircuit instance has a name, unique per kind in its (sub)circuit. Names make the
sheet paths of nets (section 5) and the UUIDs of the KiCad netlist, which pcbnew uses to keep
footprints linked to components (`doc/kicad_netlist_usage.md` section 3).

- Wrapping the part or the generator in `circuit::Named` gives an explicit name
  (`Named("amp_left", Follower)`, also around `with_options`). Names must be non-empty, must not
  contain `#`, and must be unique per kind in one (sub)circuit.
- Otherwise the instance gets an automatic name `<base>#<n>`, numbered per
  kind in each (sub)circuit: the reference prefix for parts (`R#1`, `R#2`) and
  `SubcircuitGenerator::base_name()` for subcircuits (by default the type name without module path
  and generic arguments, e.g. `RcStage#1`).

> **Note:** an automatic name changes when a part of the same kind is added or removed before it
> in the same (sub)circuit, and changing an automatic name to an explicit one changes it once.
> Either changes the UUID, so pcbnew no longer links the footprint to the component.
> Give explicit names to parts whose placement you want to keep.

Reference designators (`R1`, `R2`, ...) are numbered in instantiation order over the whole circuit.

## 5. Net names

`circuit.label(node, label)` names the net containing `node`:

| Label | Connects | Net name |
|---|---|---|
| `NetLabel::global("GND")` | every node with the same global name | `GND` |
| `NetLabel::local("OUT")` | nodes with the same local name in the same subcircuit instance | `/OUT`, `/amp_left/OUT` |
| (none) | - | `Net-(R1-Pad2)`, `Net-(U1-OUT)`, `unconnected-(U1-NC-Pad7)` |

- Sheet paths are made from the instance names of the subcircuits (`/amp_left/stage#1/`).
  `/` in names is written as `{slash}`, so `{` is not allowed in explicit names and labels.
- A net with several names is not an error (as in KiCad). The name is chosen by:
  global over local, then the label in the shallower subcircuit instance, then the smallest name.
  A `MultipleNetNames` warning is reported, and the other names still connect nets
  (a net labeled `ALPHA` and `ZED` is connected to another net labeled `ZED`).
- An unnamed net is named after one of its pins, as Eeschema does: `Net-(<ref>-<pin name>)`, or
  `Net-(<ref>-Pad<number>)` for a pin without a name or named as its pad number. A pin name shared
  by several pins of the part is followed by the pad (`Net-(U1-GND-Pad5)`). Pins written by their
  name alone are preferred, and among them the smallest name in string order (not natural order:
  `Net-(R10-Pad1)` before `Net-(R9-Pad1)`); the pin type does not matter. A net of a single pin is
  `unconnected-(...)`. Pins of parts without KiCad information are used only if no pin has it,
  by their pin IDs.
- A labeled net without any part pin is not written and gives a `LabelOnlyNet` warning.
- Two different nets with the same name are an error (`DuplicateNetName`).

SPICE node names are derived from these names (`doc/spice_netlist_usage.md` section 4).

## 6. Errors and warnings

Each error and warning carries a trace: the source location that caused it, followed by the
locations where the enclosing subcircuits were instantiated. `Display` prints them innermost first:

```
error: R3: no footprint is assigned
  at src/filters.rs:42:13
  in subcircuit instantiated at src/main.rs:18:5
```

Errors raised by `to_netlist` (`CircuitErrorKind`):

| Error | Cause |
|---|---|
| `InvalidName` | empty name or label, `#` in an explicit name, or `{` in an explicit name or label; a reference prefix (`Part::get_ref_base_name`) that is empty, ends with a digit, or contains whitespace, `?`, `#`, `/` or `{`; a `SubcircuitGenerator::base_name` used for an automatic name that is empty or contains `#` or `{` |
| `DuplicateInstanceName` | the same explicit name twice in one (sub)circuit (the note points at the first) |
| `DuplicatePinId` / `UnknownPinId` | duplicate pin IDs; unknown pin IDs in internal connections |
| `DuplicateRef` | two parts with the same reference designator |
| `DuplicateNetName` | two nets with the same name |

Errors specific to a format are listed in `doc/kicad_netlist_usage.md` section 5 and
`doc/spice_netlist_usage.md` section 4.

Warnings (`MultipleNetNames`, `LabelOnlyNet`) are kept in `Netlist::warnings` and in the ERC report,
and are also emitted with `log::warn!` (target `cuprite_wire::error`); nothing is printed unless
the program installs a logger such as `env_logger`.
Checks based on the electrical types of pins (ERC) are a separate step, `Netlist::erc`; see `doc/erc.md`.
