# Security policy

## Supported versions

Only the latest release of the crates in this repository receives fixes.

## Reporting a vulnerability

Please do not open a public issue for a security problem. Report it privately through
[GitHub's private vulnerability reporting](https://github.com/hyranno/cuprite_wire/security/advisories/new)
instead, with the steps to reproduce it and the versions of cuprite_wire, Rust and ngspice.

## Scope

cuprite_wire runs external programs and loads native code on behalf of the user: the ngspice
command line, the ngspice shared library (feature `shared-ngspice`), Verilator and the C++ compiler,
and the shared libraries of XSPICE `d_cosim` models. SPICE decks, SPICE models and Verilog sources
are trusted input, like the source code of a program: running a simulation of an untrusted model
is not expected to be safe. Reports are still welcome when ordinary input has unexpected effects,
for example files written outside the run directory.
