---
name: spice
description: Run and inspect SPICE simulations with ngspice in batch mode, and understand how KiCad 10 turns symbols (Sim.* fields) into a SPICE netlist. Use when checking a generated SPICE netlist, running an operating-point/AC/transient analysis, extracting waveform data, or researching KiCad's SPICE export rules.
---

# SPICE (ngspice) investigation

ngspice 44.2 is the simulator KiCad uses (apt packages `ngspice` and `libngspice0`).

## Running

```bash
cd $TMPDIR                                   # relative output paths (wrdata) land here
ngspice -b -o sim.log /path/circuit.cir      # deck with a .control block
ngspice -b /path/kicad_export.cir            # deck without .control: runs its dot-analyses
```

This also works inside the Claude bash sandbox. The CLI copies its input with glibc
`tmpfile()`, which always writes to `/tmp` (ignores `$TMPDIR`), and `/tmp` is read-only there;
the devcontainer's `/usr/local/bin/ngspice` wrapper preloads a shim
(`.devcontainer/tmpfile_shim.c`) that puts the file in `$TMPDIR`. If `ngspice` fails with
`tmpfile(): Read-only file system`, the container was built without it (`which ngspice`
shows `/usr/bin/ngspice`).

- Warnings (e.g. an ignored malformed element line) do **not** fail the run - read the output.
- `SpiceDeck::run` in the Rust code runs the same command (`src/spice/simulation.rs`).

## Control blocks

For machine-readable results put a `.control` block in the deck (it replaces the
dot-analysis lines when present; `run` executes any dot-analyses that remain):

```spice
.control
set filetype=ascii
set wr_singlescale        $ one shared x column instead of x/y pairs per vector
set wr_vecnames           $ header line with vector names
op
print all                 $ node voltages / branch currents at the operating point
ac dec 10 1 1Meg
wrdata ac.txt vdb(out) vp(out)
tran 1u 1m
meas tran vmax MAX v(out)
wrdata tran.txt v(out) i(v1)
quit
.endc
```

`wrdata` paths are relative to ngspice's working directory and do not expand
environment variables. Complex AC vectors are written as real/imag columns unless
you use `vdb()/vp()/vm()`.

Tips:
- Node `0` is ground; KiCad's export uses `GND`, which ngspice also treats as ground.
- KiCad node names contain `/` and `-` (`/lowpass`, `Net-_U1-+_`). In front-end
  expressions quote them: `wrdata ac.txt vdb("/lowpass")`, `let lp = vdb("/lowpass")`.
  Unquoted `vdb(/lowpass)` is a syntax error there (it happens to work in `meas`).
  `display` lists all vector names after a run.
- `.include` / `.lib` paths are relative to the current directory, not to the deck.
- Convergence trouble: `.options gmin=1e-10 reltol=1e-3 itl1=500`, or add `.ic` / `uic`.
- Vendor (PSpice/LTspice) models may need a compatibility mode in `.spiceinit`
  (`set ngbehavior=ps` or `lt`); `.spiceinit` is read from the current directory and `$HOME`.

## How KiCad builds the SPICE netlist

Reference export (see the `kicad` skill for the XDG env setup required in the sandbox):

```bash
kicad-cli sch export netlist --format spice -o $TMPDIR/out.cir \
  /usr/share/kicad/demos/simulation/laser_driver/laser_driver.kicad_sch
```

Output shape: `.title`, `.include` lines from `Sim.Library`, analysis directives taken
from schematic text items, one element line per symbol, `.end`. Net names: labels become
`/name` (hierarchical path), unnamed nets `Net-_U1-+_`, power nets their power-symbol value.

Symbol fields that drive the model (KiCad 9 and 10):

| Field | Meaning | Examples |
|---|---|---|
| `Sim.Device` | element kind | `R C L V I D NPN PNP NMOS PMOS SUBCKT SPICE IBIS` |
| `Sim.Type` | model/source subtype | `DC SIN PULSE PWL GUMMELPOON VDMOS =` (behavioral) |
| `Sim.Params` | `key=value` parameters | `dc=0 ampl=1 f=1k ac=1`, `y1=0 y2=1 td=2n ...` |
| `Sim.Library` | model file, `.include`d | `ad8051.lib` (relative to the project) |
| `Sim.Name` | model / subckt name in that library | `Q2N2222`, `AD8051` |
| `Sim.Pins` | symbol pin number -> model pin, in model order | `1=+ 2=-`, `1=C 2=B 3=E`, `2=1 1=2` |
| `Sim.Enable` / `exclude_from_sim` | drop the part from simulation | |

Without `Sim.*` fields KiCad infers the device from the reference prefix
(`R C L V I D Q M J`...) and parses `Value` (`10k`, `100n`, `DC 5`). Subcircuits
are emitted as `X<ref> nodes... <name>`; pin order follows `Sim.Pins`.

Look at real examples instead of guessing:
- `/usr/share/kicad/demos/simulation/*` - one demo per feature (sources, models, IBIS, subsheets...)
- `/usr/share/kicad/symbols/Simulation_SPICE.kicad_sym` - SPICE-only symbols (VDC, VSIN, ...);
  inspect with `python3 -I .claude/skills/kicad/scripts/kicad_lib.py sym Simulation_SPICE:VSIN`
- `/usr/share/kicad/symbols/Simulation_SPICE.sp` - KiCad's built-in subcircuits
  (`kicad_builtin_opamp`, `kicad_builtin_varistor`, ...)
- Survey all field values: `grep -rhoE '\(property "Sim\.[A-Za-z]+" "[^"]*"' --include=*.kicad_sch /usr/share/kicad/demos/simulation | sort | uniq -c | sort -rn`

Manual: https://ngspice.sourceforge.io/docs/ngspice-html-manual/manual.xhtml
