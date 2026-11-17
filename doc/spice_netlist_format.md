# SPICE Netlist Format

This document describes the SPICE netlist that cuprite_wire emits to test circuits with ngspice.
The target is ngspice 44 (the simulator bundled with KiCad 9), and the output follows the shape of
KiCad 9's own SPICE export (`kicad-cli sch export netlist --format spice`) where it makes sense,
so that the two can be compared directly.

> Sources:
>
> 1. Actual output of `kicad-cli sch export netlist --format spice` / `--format spicemodel` from KiCad 9.0.2
> 2. The behavior of the KiCad 9.0 source code (the Eeschema SPICE exporter)
> 3. Actual runs of ngspice 44.2 on hand-written decks (the results quoted here were observed in this environment)
> 4. The ngspice manual
>
> See "References" at the end for details.

## 1. Overall structure

A SPICE netlist ("deck") is a line-oriented text file.

```spice
.title my_board                         ; first line: title
.include "models/ad8051.lib"            ; model libraries
.model __D1 D(is=1n)                    ; inline models
.tran 1u 10m                            ; analysis / option directives
V1 /in GND DC 0 SIN( 0 1 1k ) AC 1      ; element lines
R1 /in /out 1k
C1 /out GND 100n
XU1 /out /fb VDD VSS /fb AD8051
.control                                ; optional: ngspice front-end script
...
.endc
.end                                    ; last line
```

KiCad writes the sections in this order:

1. `.title KiCad schematic`
2. `.include "<absolute path>"` for each model library (`Sim.Library`)
3. `.model` lines for built-in models that need one (named `__<ref>`, e.g. `.model __D1 D`)
4. Directives: the text items of the schematic starting with `.` (analyses, `.options`, `.control` blocks, ...), verbatim
5. One element line per symbol, in schematic order (not sorted)
6. `.end`

The order of element lines and `.model` lines is insignificant to ngspice;
only the first line (title) and `.end` are positional.

## 2. Lexical rules (ngspice)

| Rule | Detail |
|---|---|
| Title | The **first line is always the title** and is never parsed as a circuit line, whatever it contains. `.title` replaces it. Always emit a title line first. |
| End | `.end` terminates the deck; anything after it is ignored. |
| Comments | A line starting with `*` is a comment. `$` (preceded by whitespace) and `;` start an inline comment. `//` also starts a comment, which matters for net names (see 4). |
| Continuation | A line starting with `+` continues the previous line. |
| Separators | Whitespace, and also `,`. `(` `)` and `=` are significant in many places. |
| Case | **Case-insensitive.** Names are lowercased internally: nodes `/A` and `/a` are the same node (verified). Output vectors are reported in lowercase (`net-_c1-pad1_`). |
| Numbers | Scale suffixes `T G Meg k m u n p f` (case-insensitive; `M` = milli, `Meg` = mega). Trailing letters after a suffix are ignored (`10uF` = `10u`, `1kOhm` = `1k`). |
| Expressions | `{expr}` or `'expr'` with `.param` values; KiCad uses e.g. `R = 'TIME > 300m ? 7 : 8'` for behavioral resistors. |

## 3. Element lines

```
<name> <node1> <node2> ... [<model name>] [<value / params>]
```

The **first letter of the name selects the device type**. Typical letters:

| Letter | Device | Line form (nodes in this order) |
|---|---|---|
| `R` | resistor | `R1 n+ n- 10k` |
| `C` | capacitor | `C1 n+ n- 100n` (optionally `ic=...`) |
| `L` | inductor | `L1 n+ n- 10u` |
| `K` | coupled inductors | `K1 L1 L2 0.99` |
| `V` / `I` | independent source | `V1 n+ n- DC 5 AC 1 SIN( ... )` (current flows from n+ through the source to n-) |
| `E` `F` `G` `H` | controlled sources | `E1 n+ n- nc+ nc- gain` |
| `B` | behavioral source | `B1 n+ n- V = {v(a)*2}` |
| `D` | diode | `D1 anode cathode model` |
| `Q` | BJT | `Q1 C B E [S] model` |
| `M` | MOSFET (incl. VDMOS) | `M1 D G S [B] model` (VDMOS: `D G S`) |
| `J` | JFET | `J1 D G S model` |
| `S` / `W` | switches | `S1 n+ n- nc+ nc- model` |
| `T` | transmission line | `T1 p1+ p1- p2+ p2- z0=50 td=1n` |
| `X` | subcircuit instance | `XU1 node1 node2 ... SUBCKT_NAME [param=val ...]` |
| `A` | XSPICE code model | `A1 [in] [out] model` |

### 3.1 Element naming (KiCad's rule)

KiCad emits the reference as the element name, and **prepends the device letter when the
reference does not already start with it** (case-sensitive check):

| Reference | Device | Element name |
|---|---|---|
| `R1` | R | `R1` |
| `Rload1` | R | `Rload1` |
| `U1` | subcircuit | `XU1` |
| `Q2` | NMOS / VDMOS | `MQ2` |
| `Q9` | subcircuit | `XQ9` |

A mismatch between the letter and the model type is an error in ngspice
(e.g. `Q1 ... model` with a VDMOS model: `incorrect model type`), so the letter must be derived
from the SPICE model of the part, not from its reference designator. Reference uniqueness
then guarantees element-name uniqueness (case-insensitively; avoid references differing only in case).

### 3.2 Pin order

Nodes are written in the **model's pin order**, not in symbol pin number order.
KiCad maps symbol pins to model pins with the `Sim.Pins` field
(e.g. `1=C 2=B 3=E`, `1=K 2=A` for a diode whose symbol pin 1 is the cathode, `2=1 1=2` to swap).
For subcircuits the order is that of the `.subckt` line in the library.
cuprite_wire part definitions must carry the same mapping (part pin -> SPICE node position).

### 3.3 Values and sources

KiCad derives the value either from `Sim.Device`/`Sim.Type`/`Sim.Params`, or, without `Sim.*`
fields, from the reference prefix and the `Value` field (`10k`, `100n`, `DC 5`).
Forms observed in the KiCad 9 output:

| Kind | Example output |
|---|---|
| passive | `R1 /out GND 1k` |
| DC source | `V2 VDD GND DC 10` |
| AC small-signal | `V1 n GND DC 0 AC 1` |
| sine | `V7 n GND DC 0 SIN( 0 1 100Meg 1n 10G )` (`SIN( vo va freq td theta phase )`) |
| pulse | `V4 n GND PULSE( -1 1 2n 30n 2n 50n 100n )` (`v1 v2 td tr tf pw per`) |
| exponential | `V3 n GND EXP( -4 -1 2n 30n 60n 40n )` |
| piecewise linear | `V4 n GND PWL( 0 -1.2 10m -1.2 10.01m -1.6 )` (time/value pairs) |
| noise | `V8 n GND TRNOISE( 100m 500p 0 0 0 0 0 )`, `TRRANDOM( ... )` |
| model reference | `D1 a k 1N4148`, `Q1 c b e Q2N2222` |
| subcircuit with params | `XU1 a b c d e kicad_builtin_opamp POLE=30 GAIN=100k VOFF=10u ROUT=10` |
| behavioral R | `Rload1 a GND R = 'TIME > 350m ? 8 : 89'` |

Note: KiCad copies the `Value` text as is when no `Sim.*` fields exist, so values like
`100uF;16V` appear in its output. ngspice happens to accept them (`;16V` is an inline comment and
the `F` after the suffix is ignored), but cuprite_wire should emit clean numeric values.

## 4. Nodes (nets)

- Node names are arbitrary tokens. KiCad uses the net names of the `.net` export
  (see `kicad_netlist_format.md`, 6.1), converted as follows:
  - Eeschema escapes (`{slash}` etc.) are unescaped first.
  - The characters `% ( ) , [ ] < > ~` and space are replaced by `_`
    (so `Net-(R1-Pad2)` becomes `Net-_R1-Pad2_`, `Net-(U1-+)` becomes `Net-_U1-+_`).
  - A name starting with `//` is rewritten to start with `/root/`, since `//` opens a comment.
  - An unconnected pin with no net name gets `NC-<n>` (n counts from 1). Pins in the `.net` always
    have a net (`unconnected-(R3-Pad2)` for a single pin), and KiCad 9.0.2 writes those as
    `unconnected-_R3-Pad2_` (observed with `tools/compare_kicad_cli_netlist.py --spice`).
- Characters that must not appear in a node name, found by testing ngspice 44.2:
  - `,` — it is a separator (`c,d` is read as two tokens).
  - `=` — it is parsed as a parameter assignment (`x=y` gives `Undefined parameter [y]`).
  - `(` `)` — confuse the parser in many contexts.
  - `{` `}` `'` — expression delimiters: the deck fails to load.
  - `"` — the name is split at it.
  - `;` anywhere — starts an inline comment, the rest of the line is lost.
  - whitespace, a leading `$` (after whitespace it starts a comment), and a leading `//`.
  - Non-ASCII characters are replaced (`/é` becomes `/__`), so different names can be merged.

  A leading `*` or `+` only matters at the start of a line, so `+5V` and `*a` work as nodes.
  `#` `:` `@` `.` `!` `&` `|` `\` `?` and a `$` inside a name also work.
  Hierarchical names with `/` and `-` (`/amp/in`, `Net-_U1-+_`) work as element nodes.
- Because ngspice is case-insensitive, two different net names that differ only in case
  are **silently merged** into one node. cuprite_wire should reject such pairs (or make names unique).
- Ground: node `0`. ngspice also treats `GND` (any case) as ground; KiCad emits `GND` or `0`
  depending on the power symbol's value. Every circuit needs a ground reference.
- Floating nodes / no DC path to ground: ngspice 44 falls back to gmin/source stepping and
  "transient op", and may report a plausible-looking operating point instead of failing.
  This should be caught by our own checks (ERC) before simulation rather than relied on in ngspice.

## 5. Models and subcircuits

- `.include "path"` inserts a file verbatim. **Relative paths are resolved against ngspice's
  current working directory, not the deck's directory.** KiCad therefore emits absolute paths.
- `.lib "path" section` includes one section of a library file.
- `.model NAME TYPE( param=value ... )` defines a device model
  (types: `D NPN PNP NMOS PMOS NJF PJF VDMOS SW CSW R C L` ...; VDMOS takes `NCHAN`/`PCHAN`).
  KiCad emits `.model __<ref> TYPE ...` for parts using a built-in model with parameters
  (e.g. `.model __Q2 VDMOS NCHAN`, `.model __D2 D`), referenced from the element line.
- A subcircuit is defined as

  ```spice
  .subckt NAME port1 port2 ... [param=default ...]
  ...elements, using port names as nodes...
  .ends [NAME]
  ```

  and instantiated with `X<name> node1 node2 ... NAME [param=value ...]`.
  Node names inside a subcircuit are local; node `0` is global.
- KiCad ships its own generic subcircuits in `/usr/share/kicad/symbols/Simulation_SPICE.sp`
  (e.g. `kicad_builtin_opamp`), referenced by symbols from `Simulation_SPICE.kicad_sym`.
- Vendor models written for PSpice/LTspice may need a compatibility mode
  (`set ngbehavior=ps` / `lt` in `.spiceinit`).

### 5.1 Subcircuit export (`spicemodel`)

`kicad-cli ... --format spicemodel` writes the same items wrapped in a subcircuit
(`.subckt <sheet name>` ... `.ends`) with directives (except the simulation commands) placed inside.
A cuprite_wire subcircuit could be exported the same way, with its ports listed on the `.subckt` line,
so that it can be reused from other decks.

## 6. Analyses and the control block

Dot analyses (one per line, executed by `run` or in batch mode):

| Directive | Meaning |
|---|---|
| `.op` | DC operating point |
| `.dc V1 0 5 0.1` | DC sweep of a source |
| `.ac dec 10 1 1Meg` | small-signal AC sweep (needs an `AC` magnitude on a source) |
| `.tran tstep tstop [tstart [tmax]] [uic]` | transient |
| `.noise`, `.tf`, `.pz`, `.sens`, `.disto` | other analyses |
| `.options ...` | simulator options (`reltol`, `abstol`, `gmin`, `method=gear` ...) |
| `.ic v(node)=val`, `.nodeset` | initial conditions / hints |
| `.meas tran NAME ...` | measurements printed after the analysis |
| `.save`, `.probe` | which vectors to keep (KiCad emits `.save all`, `.probe alli`, `.probe p(R1)` as options) |

For automated tests a `.control` ... `.endc` block (ngspice front-end script) is the most useful,
since it can run analyses in sequence and write machine-readable output:

```spice
.control
set filetype=ascii
set wr_singlescale              $ one shared x column
set wr_vecnames                 $ header line with vector names
ac dec 10 1 1Meg
meas ac f3db WHEN vdb("/out")=-3
wrdata ac.txt vdb("/out") vp("/out")
tran 1u 1m
meas tran vmax MAX v("/out")
wrdata tran.txt v("/out") i(V1)
op
print all
quit
.endc
```

Notes (verified with ngspice 44.2):

- In front-end commands, quote node names containing `/` or `-`: `v("/out")`, `vdb("Net-_U1-+_")`.
- `meas` and `wrdata` work on the **current plot** (the last analysis run).
  Running `op` and then `meas ac ...` fails (`measure limited to tran, dc, sp, or ac analysis`);
  measure right after each analysis, or switch with `setplot`.
- Branch currents are available only for voltage sources (`i(V1)`, vector `v1#branch`).
  To measure a current elsewhere, insert a 0 V source in series.
- `wrdata` output: a header line (with `wr_vecnames`), then whitespace-separated columns,
  the first being the scale (time/frequency); paths are relative to the working directory.
- Errors are printed as `Error ...` lines; warnings do not stop the run. A test harness should
  treat any `Error` line as a failure.

KiCad copies `.control` blocks from schematic text items verbatim; cuprite_wire tests can instead
append a generated control block to the netlist body.

## 7. Minimal form

```spice
.title rc_lowpass
V1 /in 0 DC 0 AC 1
R1 /in /out 1k
C1 /out 0 1u
.ac dec 10 10 100k
.end
```

## 8. Output policy for cuprite_wire

Implemented in `src/spice/` (usage: `doc/spice_netlist_usage.md`).

- The netlist has a title line, `.include` lines (absolute paths, sorted), inline `.model`/`.subckt`
  definitions (sorted by name), the element lines, and `.end`. Analyses and `.control` blocks are
  supplied by the test (`SpiceDeck`), not by the circuit, so the same circuit can be simulated with
  different analyses and stimuli. Stimuli are simulation-only parts in a test bench circuit.
- Elements follow the component order of the netlist (subcircuit instance by instance, then by
  reference). KiCad uses the schematic order; ngspice does not care.
- Element names: SPICE device letter + reference (+ a suffix for parts with several elements),
  prepending the letter only when the reference does not already start with it (same rule as KiCad).
- Node names: the net names of the `.net` output, converted by KiCad's rule (section 4), so that
  the output can be cross-checked against KiCad's export of an equivalent schematic
  (`tools/compare_kicad_cli_netlist.py --spice`). Names ngspice cannot read and names that collide
  case-insensitively are errors.
- Ground: a net named `GND` (any case) or `0` is written as `0`; a netlist without one is an error.
- Each part carries its SPICE description (`SpicePartInfo`): device letter, nodes in model order
  (part pin IDs), arguments, and model files or inline definitions -- the counterpart of
  `Sim.Device` / `Sim.Library` / `Sim.Name` / `Sim.Params` / `Sim.Pins`. Parts without one are an
  error; parts not to be simulated (connectors, mechanical parts, ...) are excluded explicitly
  (`SpicePartInfo::Excluded`, like `exclude_from_sim`). Every pin must be used by the model or
  declared unused.
- Arguments with `;` or `$` are rejected, so values are clean (`100u`, not `100uF;16V`).
- Running: `ngspice -b -o sim.log deck.cir` from a shell. Inside the Claude sandbox use
  `tools/ngspice_run.py` (see `.claude/skills/spice/SKILL.md`); `SpiceDeck::run` uses it by default.

## References

| # | Source | Used for | License |
|---|---|---|---|
| 1 | Actual output of KiCad 9.0.2 `kicad-cli sch export netlist --format spice` / `spicemodel` (run on the bundled demos under `demos/simulation`) | Section order, element/node naming, source syntax | The output itself is not reproduced. Examples in this document were written for this project, with value forms reduced to the syntax. |
| 2 | KiCad source `eeschema/netlist_exporters/netlist_exporter_spice.cpp` (9.0 branch) <https://gitlab.com/kicad/code/kicad/-/blob/9.0/eeschema/netlist_exporters/netlist_exporter_spice.cpp> | Output order, net-name character replacement, `//` handling, `NC-<n>` naming | GPL-2.0-or-later |
| 3 | KiCad source `eeschema/sim/spice_generator.cpp` (9.0 branch) <https://gitlab.com/kicad/code/kicad/-/blob/9.0/eeschema/sim/spice_generator.cpp> | Element-name prefixing rule | GPL-2.0-or-later |
| 4 | ngspice 44.2 (`libngspice0`), runs of test decks written for this document | Case-insensitivity, forbidden characters, floating-node behavior, `meas`/`wrdata` behavior | The tests and results are this project's own. |
| 5 | ngspice User's Manual <https://ngspice.sourceforge.io/docs/ngspice-html-manual/manual.xhtml> | General syntax, element and analysis forms | Manual of the ngspice project; text is not reproduced |

### License notes

- This document does not reproduce code or text from the sources above; it describes facts
  (the syntax of SPICE and the observable behavior of the tools) independently.
- **Do not copy or port KiCad source code** into the implementation (the GPL would propagate).
- Do not commit reference outputs generated from the KiCad demos or their model libraries
  (the vendor `.lib` files have their own licenses). Create test circuits and expected outputs in this project.
- Vendor SPICE models used in tests must be checked for redistribution terms before being committed.
