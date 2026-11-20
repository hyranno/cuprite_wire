# Contributing to cuprite_wire

Thank you for your interest in cuprite_wire. Bug reports, questions, documentation fixes and code
are all welcome. This document describes how to contribute and the rules contributions must follow.

## Reporting issues

When reporting a bug, please include:

- a minimal circuit (Rust code) that reproduces it,
- what you expected and what happened (error messages, ERC output, the generated netlist),
- the versions of cuprite_wire, Rust, KiCad and ngspice.

Do not attach files you are not allowed to share, such as vendor SPICE models with restricted
redistribution (see [Licenses](#licenses)). Refer to them by name and where to download them instead.

## Development environment

Using the devcontainer in `.devcontainer/` is recommended. It has everything needed to build and
test: Rust, the KiCad 9 libraries and pcbnew, and ngspice. CI uses the same base image
(`rust:trixie`). You may use your own environment or a customized devcontainer instead, as long as
the checks below pass.

If a change needs a new tool on the machine, adding it to the devcontainer is recommended, so that
others get the same environment.

## Making changes

1. `main` is protected. Create a topic branch and open a pull request.
2. Keep a pull request focused on one topic. Update the documentation (`README.md`, `doc/`) together
   with the behavior it describes.
3. Run the same checks as CI before opening the pull request:

   ```sh
   cargo fmt --all --check
   cargo clippy --workspace --all-targets --features shared-ngspice -- -D warnings
   cargo clippy -p cuprite_wire --all-targets -- -D warnings
   cargo test --workspace --features kicad-libraries,shared-ngspice
   cargo test -p cuprite_wire_kicad_lib -- --ignored
   ```

   The `shared-ngspice` tests and `cuprite_wire_embedded_hal` need the ngspice shared library
   (`libngspice0`, in the devcontainer).

   The code is formatted with the default rustfmt settings.

4. If a change intentionally alters output, regenerate the expected files and review the diff:
   `UPDATE_GOLDEN=1 cargo test` for the golden netlists in `tests/golden/`, and
   `TRYBUILD=overwrite cargo test --test kicad_part_errors` for the compile-error messages.
5. Tests with real vendor SPICE models are opt-in and not run by CI:
   `CUPRITE_VENDOR_MODELS_DIR=/path/to/models cargo test --features vendor-models --test vendor_models`
   (see `doc/spice_part_macro.md`).

## Developer documentation

| Document | Contents |
|---|---|
| [`doc/dev/kicad_netlist_format.md`](doc/dev/kicad_netlist_format.md) | the KiCad netlist format, as observed in KiCad 9 |
| [`doc/dev/spice_netlist_format.md`](doc/dev/spice_netlist_format.md) | the SPICE netlist format and ngspice's behavior |
| [`doc/dev/verifying.md`](doc/dev/verifying.md) | the tests and the scripts in `tools/` that check the output against KiCad and ngspice |

The user documentation is listed in [`README.md`](README.md#documentation).

## Language

- Write code comments, commit messages, pull requests and documents (`README.md`, files under
  `doc/`, etc.) in English.
- Localized versions of documents (e.g. `README.ja.md`) are written in their respective languages.

## Authorship and AI assistance

The author of a commit is the person who takes responsibility for it: who understands the change,
can answer questions about it, and maintains it.

- **Commits must have a human author.** Commits authored only by an AI tool will not be merged.
  Using AI tools to write code or documents is fine, but a human must review the result and commit
  it under their own name.
- **Credit AI assistance as a co-author.** When an AI tool contributed substantially to a commit,
  add a `Co-Authored-By:` trailer for it to the commit message, for example:

  ```text
  Co-Authored-By: Claude <noreply@anthropic.com>
  ```

  This is recommended, not enforced; a forgotten trailer is not a reason to reject a contribution.
- **Do not credit AI tools anywhere else.** Leave such notes out of code comments, documentation,
  file headers and generated files. The commit trailer is the only place for them.
- **Single-purpose bots** such as Dependabot may author their own commits. The maintainer who
  merges them takes responsibility for the change.

## Licenses

cuprite_wire is licensed under the [MIT License](LICENSE). By contributing, you agree that your
contribution is licensed under the same terms, and you confirm that you have the right to submit it.

Respect the licenses of everything the project touches. In particular:

- **Do not copy or port KiCad source code** (GPL). Reading it to understand the behavior is fine;
  write the implementation independently.
- **Do not commit KiCad library files** (symbols, footprints, `Simulation_SPICE.sp`) or excerpts of
  them. `kicad_part!` reads them at compile time.
- **Do not commit outputs generated from the KiCad demos** or other third-party projects. Create
  test circuits and expected outputs in this project.
- **Do not commit vendor SPICE models** unless their terms clearly allow redistribution. Write test
  models for the project instead, as in `crates/kicad_lib/tests/fixtures/models/`.
- **Do not quote or reproduce sources without a license statement.** Describe facts (e.g. the
  structure of a file format) in your own words, and list the sources you used, with their licenses,
  in the references of the document, as in `doc/dev/kicad_netlist_format.md`.
- **New dependencies** must have licenses compatible with MIT (e.g. MIT, Apache-2.0, BSD).
- **AI-generated content** is subject to the same rules. Make sure it does not reproduce code or
  text from sources with incompatible licenses.

The details are in the license sections of `doc/kicad_part_macro.md`, `doc/spice_part_macro.md`,
`doc/dev/kicad_netlist_format.md` and `doc/dev/spice_netlist_format.md`.
