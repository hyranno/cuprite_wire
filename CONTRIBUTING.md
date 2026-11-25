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
test: Rust, KiCad 10 (its libraries, `kicad-cli` and pcbnew), ngspice (the CLI and the shared
library), gcc/g++ and Verilator for the XSPICE tests, Xvfb for the pcbnew import check
(`doc/dev/verifying.md`), and cargo-deny for the dependency checks. It is based on the
`kicad/kicad:10.0` image. CI runs the tests with
KiCad 10 on the same image and with KiCad 9 on `rust:trixie` (Debian's KiCad libraries). You may use your own environment or a customized
devcontainer instead, as long as the checks below pass.

If a change needs a new tool on the machine, adding it to the devcontainer is recommended, so that
others get the same environment.

## Making changes

1. `main` is protected. Create a topic branch and open a pull request.
2. Keep a pull request focused on one topic. Update the documentation (`README.md`, `doc/`) together
   with the behavior it describes, and add user-visible changes to the `[Unreleased]` section of
   [`CHANGELOG.md`](CHANGELOG.md) (mark breaking changes with **Breaking:**).
   Public items of `cuprite_wire`, `cuprite_wire_embedded_hal` and `cuprite_wire_ibis` need doc
   comments, with `# Errors` / `# Panics` sections where they apply (clippy checks this).
3. Run the same checks as CI before opening the pull request:

   ```sh
   cargo xtask all
   ```

   It checks the formatting (the default rustfmt settings), runs clippy and rustdoc with warnings
   denied, checks the licenses and sources of the dependencies with
   [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) (`deny.toml`), and runs the tests,
   including the ignored tests reading the KiCad libraries and demos. `cargo xtask help` lists
   the tasks and their options (e.g. `cargo xtask test --no-kicad` without a KiCad
   installation). The commands are in `xtask/src/main.rs`; the CI workflow runs the same tasks, and
   also `cargo xtask check` with the minimum supported Rust (`rust-version` in `Cargo.toml`), which
   `all` does not include.

   The git hooks in `.githooks/` run these checks for you: `pre-commit` checks the formatting and
   `pre-push` runs `cargo xtask all` (`SKIP_TESTS=1 git push` skips the tests,
   `git push --no-verify` all the checks). Enable them with
   `git config core.hooksPath .githooks`; the devcontainer does this when it is created. Since the
   checks run on the working tree, `pre-push` refuses to push a branch that is not checked out or
   has changes that are not committed.

   The XSPICE tests building code models run on Linux only (see the platform notes in
   `README.md`); see `.github/workflows/ci.yml` for how CI installs ngspice on Windows and macOS.

4. If a change intentionally alters output, regenerate the expected files (golden netlists and
   compile-error messages) and review the diff. The commands, and the opt-in tests with real
   vendor SPICE models that CI does not run, are in `doc/dev/verifying.md` section 1.

## Refactoring aids

The devcontainer also has these tools. CI does not run them, and their findings are hints to
review, not rules.

- `similarity-rs --skip-test -m 10 -t 0.9 .` lists similar functions. Many hits are thin
  wrappers or trait implementations in tests; look for the longer ones.
- `cargo shear` finds unused or misplaced dependencies and source files that no module includes.
- `cargo public-api -p cuprite_wire --simplified` lists the public API (it switches to the nightly
  toolchain for rustdoc JSON by itself). Use it to spot items that should not be public. To compare
  with a release, save the list for both versions and diff the files; its own `diff` subcommand
  checks out each commit in the working tree.
- `cargo semver-checks --workspace --exclude cuprite_wire_kicad_lib` compares the public API with
  the latest release on crates.io and reports breaking changes (before 1.0, "major" means the
  minor version). `cuprite_wire_kicad_lib` is exempt from semver; exclude a crate that has never
  been published, too. It checks with all the features enabled.

## Developer documentation

| Document | Contents |
|---|---|
| [`doc/dev/kicad_netlist_format.md`](doc/dev/kicad_netlist_format.md) | the KiCad netlist format, as observed in KiCad 9 and 10 |
| [`doc/dev/kicad_bom_format.md`](doc/dev/kicad_bom_format.md) | the BOM (CSV) of `kicad-cli sch export bom`, as observed in KiCad 10 |
| [`doc/dev/spice_netlist_format.md`](doc/dev/spice_netlist_format.md) | the SPICE netlist format and ngspice's behavior |
| [`doc/dev/ibis.md`](doc/dev/ibis.md) | KiCad's IBIS conversion, how to reproduce it, and the design of the IBIS support |
| [`doc/dev/design.md`](doc/dev/design.md) | design decisions about the API: what belongs to part types and what to instance overrides |
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
  `cargo xtask deny` checks them against the licenses allowed in `deny.toml`.
- **AI-generated content** is subject to the same rules. Make sure it does not reproduce code or
  text from sources with incompatible licenses.

The details are in the license sections of `doc/kicad_part_macro.md`, `doc/spice_part_macro.md`,
`doc/dev/kicad_netlist_format.md` and `doc/dev/spice_netlist_format.md`.
