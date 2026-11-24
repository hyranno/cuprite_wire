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

Note: KiCad converts values from its notation (SI prefixes with `M` for mega, the RKM code) when
it writes the netlist: `4k7` -> `4.7k`, `2R2` -> `2.2`, `1M` -> `1Meg`, `1e3` -> `1k`, `100nF` -> `100n`
(checked with kicad-cli 9). `Sim.Params` values get the prefixes converted (`r=1M` -> `1Meg`) but not
the RKM code (`r=4k7` stays `4k7`, which ngspice reads as 4k). Text it cannot parse is copied as is,
so values like `100uF;16V` appear in its output; ngspice happens to accept them (`;16V` is an inline
comment and the `F` after the suffix is ignored), but cuprite_wire emits clean numeric values.

#### Sources (`Sim.Device` `V` / `I`)

Observed with kicad-cli 10.0.8 by editing the `Sim.*` fields of the sources in a copy of
`demos/simulation/v_i_sources` and exporting it (`kicad-cli sch export netlist --format spice`);
the rules match `SPICE_GENERATOR_SOURCE::ItemLine` and `SIM_MODEL_SOURCE::doSetParamValue` in
`eeschema/sim/sim_model_source.cpp` (read for reference only; KiCad is GPL-3.0).

- Each `Sim.Type` has a fixed list of parameters, followed by `ac` and `ph`:
  `DC` `dc`; `SIN` `dc ampl f td theta phase`; `PULSE` `y1 y2 td tr tf tw per np`;
  `EXP` `y1 y2 td1 tau1 td2 tau2`; `AM` `vo vmo vma fm fc td phasem phasec`;
  `SFFM` `vo va fm mdi fc phasem phasec`; `PWL` `pwl` (a quoted list of time/value pairs).
  An unknown parameter name makes KiCad give up and write `Sim.Params` as is after the nodes
  (`V9 n GND freq=1k`), which ngspice rejects. Names are case-insensitive, `Sim.Type` is not
  (`pulse` is not found). A parameter given twice: the last one wins.
- Setting a parameter fills every empty parameter before it with `0` (with the default of the
  parameter being set when that is a number, which is `0` for all of the above), and the
  parameters after the last one given are left out: `ampl=2 f=1k` gives `SIN( 0 2 1k )`,
  `tau2=3` gives `EXP( 0 0 0 0 0 3 )`, and `ac=1` fills the whole waveform.
- Written: `DC <dc> ` when `dc` is set (for `SIN`, where `dc` is also the offset, whenever any
  parameter is given), then `<FUNC>( <values> ) ` with the values set, then `AC <ac> <ph>` when
  `ac` is set (so `ph=30` alone gives `AC 0 30`). `DC` without `dc` takes the `Value` field
  (`DC 2.345 AC 0 2` for `ph=2`), copied as is when it is not a number (`DC VTRRANDOM`).
  Without parameters a waveform is empty (`SIN( )`), which ngspice cannot read.
- `PULSE` without `per` and `np` (KiCad 10, for ngspice 46, whose default `per` turns one pulse into
  a train): KiCad appends `0 1` (per 0, np 1) after the values set. With fewer than six values set
  they land on the wrong parameters (`y1=3` gives `PULSE( 3 0 1 )`, `td` 1), and a filled `per`
  (`ac=1`) counts as set (`PULSE( 0 1 0 0 0 0 0 0 ) AC 1`). cuprite_wire writes what KiCad means:
  the values up to `tw` and `0 1` whenever neither `per` nor `np` is given.
- ngspice 44.2 reads `0` for `tr`, `tf`, `pw`, `per` of `PULSE` as their defaults (`tstep`,
  `tstop`), and accepts the eighth `np`.
- Numbers get the SI prefixes converted (`1M` -> `1Meg`) but not the RKM code (`4k7` stays), as for
  other `Sim.Params`; `PWL` values are converted one by one, and `{...}` expressions are kept.
- The noise and random types (`WHITENOISE` -> `TRNOISE( rms dt 0 0 0 0 0 )`, `PINKNOISE`,
  `BURSTNOISE`, `RANDUNIFORM` / `RANDGAUSSIAN` / `RANDEXP` / `RANDPOISSON` -> `TRRANDOM( <n> ... )`)
  are written from named parameters with their own defaults; cuprite_wire does not support them.
  The library's `VAM` / `VSFFM` / `IAM` / `ISFFM` symbols use `Sim.Device SPICE`, not these types.

## 4. Nodes (nets)

- Node names are arbitrary tokens. KiCad uses the net names of the `.net` export
  (see `doc/dev/kicad_netlist_format.md` section 6.1), converted as follows:
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
- The same holds for the files named by parameters of XSPICE code models (such as the state file
  of `d_state`): ngspice 44.2 opens them relative to its working directory, even when
  `sourcepath` is set. cuprite_wire makes them absolute when it copies a `.subckt`
  (`sim_model: inline`, `doc/spice_part_macro.md`).
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
- Errors are printed as `Error ...` lines (`Error: unknown subckt: ...`, `Error: measure  half
  when(WHEN) : out of interval`); warnings do not stop the run. A test harness should treat any
  line starting with the word `Error` as a failure, but not a value whose name starts with it
  (`meas tran error_max ...` prints `error_max = 1.0e+00`), nor `PPerror: ...`, which is followed by
  an `Error` line.
- Without a `.control` block, `ngspice -b` runs the analyses of the dot directives and prints the
  `.meas` results. With one, only the commands of the block run
  (`Note: Simulation executed from .control section`).
- With `wr_singlescale`, `wrdata` after `op` writes the first vector of the plot (a node voltage) as
  the scale column, before the requested vectors.
- The header of `wrdata` keeps the case of the expression but drops the quotes (`v("/OUT")` is
  written as `v(/OUT)`), and a complex vector (AC) is written as two columns under the same name.
  `real(v("/OUT")) imag(v("/OUT"))` writes the parts as two named real columns.
- `wrdata` reads its vectors as one expression list: `v(a) -v(b)` is the single vector
  `v(a)-v(b)`. Parenthesize a negated vector: `(-v(b))`.
- `$` in a node name starts a variable in front-end commands even inside quotes
  (`v("/a$b")` fails with `Error: b: no such variable.`), so such a node cannot be probed.
  The ground `0` has no vector (`v(0)`: `Error: no such vector 0`).
- `meas ... WHEN` takes a vector or `vdb(<node>)`, not an expression like `db(v("/out"))`
  (`Error: no such vector as db(v("/out"))`).
- `write` without a file writes `rawspice.raw` in the working directory.
- A lossless transmission line (`T1 p1 n1 p2 n2 z0=50 td=1n`) does not limit the time step by
  its delay: a `tran` gave the same number of points with `td` of 1 ps and of 10 ns.

KiCad copies `.control` blocks from schematic text items verbatim; cuprite_wire tests can instead
append a generated control block to the netlist body.

### 6.1 The shared library (libngspice)

Verified with libngspice 44.2 (`libngspice0`, `ngSpice_Init` and `ngSpice_Command`):

- `source <path>` reads a deck and runs its `.control` block, as `ngspice -b` does. `quit` in the
  block calls the `ControlledExit` callback instead of returning, so a deck for the library has none.
  Unlike `ngspice -b`, it does not run the dot analyses of a deck without a `.control` block;
  `run` after it does, and prints the `.meas` results.
- The library runs in the working directory of the process (`cd` would change it for every thread),
  so relative `wrdata` paths land there. Paths with quotes are taken literally (`"a b/x.txt"`
  fails with `"a b/x.txt": No such file or directory`, which is not an `Error` line), so paths
  with whitespace cannot be written.
- `set ngbehavior=psa` before `source` selects the mode (`Note: Compatibility modes selected: ps a`),
  and `unset ngbehavior` goes back to the default mode for the next deck. `set ngbehavior=` (empty)
  is an error.
- Output arrives line by line in the `SendChar` callback, prefixed with `stdout ` or `stderr `.
- A syntax error in an expression prints `Error: ngspice.dll cannot recover and awaits to be reset or
  detached`, calls `ControlledExit` (status 1) and makes `ngSpice_Command` return 1. Unloading the
  library (`dlclose`) and loading it again prints the banner again and runs the next deck normally.
- `remcirc` and `destroy all` free the circuit and the plots after a run, but the front-end
  variables stay: `option interp` in one run made the next run send interpolated points. The
  library is therefore loaded for each run and unloaded after it.
- With a `tstart` in `tran`, `SendData` starts at the first point after `tstart`; with
  `option interp`, it sends the interpolated points, the same time many times (and the breakpoints
  asked for print `breakpoint in the past`). Co-simulation rejects both.

### 6.2 Node `gnd` and XSPICE bridges

Verified with ngspice 44.2:

- A node named `gnd` is the ground (node 0) everywhere, also as a port of a subcircuit:
  in `.subckt sub gnd`, elements on `gnd` connect to the ground, not to the node the port is
  connected to. Generated subcircuits name their reference port `ref` instead.
- `dac_bridge` ignores a differential output (`[%vd(out ref)]`): its output is relative to the
  ground. cuprite_wire puts unity-gain `E` sources around the bridges instead.
- `d_cosim` takes `NULL` for a missing port vector; an empty vector `[]` is an error
  (`Array connection must have at least one port`). Its model parameters are `simulation`,
  `sim_args`, `lib_args`, `delay`, `queue_size` and `irreversible`.
- A model with `method = Normal` and no inputs is still stepped at every time point, with
  `vtime` set to the time it may advance to.
- Verilog through Verilator (Verilator 5.032): `vlnggen` runs `verilator --Mdir <obj> --prefix Vlng
  --CFLAGS -fpic --cc <sources>`, turns the `VL_IN8(&clk,0,0);` / `VL_OUT8(&q,1,0);` /
  `VL_INOUT...` declarations of `Vlng.h` into lines `VL_DATA(8,clk,0,0)` (no `;`) of `inputs.h`,
  `outputs.h` and `inouts.h` in `<obj>`, builds with `--build --exe verilator_main.cpp
  verilator_shim.cpp` and links `verilator_shim.o verilated.o verilated_threads.o Vlng__ALL.a` with
  `g++ --shared`. The shim numbers the bits of each port from its most significant bit, inouts after
  the outputs, and sets bits with `1 << n` on `int`, so ports wider than 32 bits do not work.
- Icarus Verilog through `ivlng.so` does not work with Debian trixie's packages: the `ivlng.so` of
  ngspice 44.2 has no dynamic symbols (`d_cosim` reports `undefined symbol: Cosim_setup`), and the
  `iverilog` 12 package has no `libvvp.so`, which the shim loads.

### 6.3 Callbacks for co-simulation

Verified with libngspice 44.2 (`ngSpice_Init` with `SendData` / `SendInitData`,
`ngSpice_Init_Sync` with `GetVSRCData`, `ngSpice_SetBkpt`):

- A voltage source `V1 a b DC 0 EXTERNAL` takes its value from `GetVSRCData(&value, time, name, ...)`.
  Inside a subcircuit the name has the instance path, in lowercase: `VPA0` in `XU1` is `v.xu1.vpa0`.
- `GetVSRCData` is called about twice per accepted point (Newton iterations and trial points), but
  never with a time before the last accepted point.
- `SendInitData` is called when a plot starts, with its type (`op1`, `tran1`) and vectors;
  `SendData` is called for every accepted point with the values of all vectors: node voltages
  named by the node in lowercase (`/line` for `/LINE`, `xu1.d` inside `XU1`), branch currents
  (`v.xu1.vpa0#branch`) and, in a transient analysis, the scale `time`. A deck running `op` and then
  `tran` gives two plots.
- `ngSpice_SetBkpt(time)` puts a time point at `time`. It works from inside `SendData`; before a
  circuit is loaded it fails with `Error: no circuit loaded.`. Without a breakpoint, a source
  changed at an accepted point only takes effect from the next one: a step meant for 1 us landed
  about 8 ns late with `tran 10n`, and exactly on time with a breakpoint.
- The callbacks run in the thread that called `ngSpice_Command` (the run is not in the background
  thread unless `bg_run` is used).

## 7. Minimal form

```spice
.title rc_lowpass
V1 /in 0 DC 0 AC 1
R1 /in /out 1k
C1 /out 0 1u
.ac dec 10 10 100k
.end
```

## 8. What cuprite_wire writes

See `doc/spice_netlist_usage.md` for the netlist cuprite_wire writes (section 6), its node names
(section 4) and running ngspice (section 5), and `doc/dev/verifying.md` for the comparison with KiCad's
export.

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
- Do not commit KiCad library files (symbols, footprints, `Simulation_SPICE.sp`) or excerpts of them;
  `kicad_part!` reads them at compile time (see `doc/kicad_part_macro.md` section 6).
