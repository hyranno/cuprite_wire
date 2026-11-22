---
name: release
description: Release a new version of the cuprite_wire workspace crates (cuprite_wire, cuprite_wire_macros, cuprite_wire_kicad_lib, cuprite_wire_embedded_hal, cuprite_wire_ibis) to crates.io and GitHub. Use when preparing or performing a release - version bump, goldens, CHANGELOG, package checks, the release PR and the tag - or when checking the public API against the previous release.
---

# Releasing cuprite_wire

All crates of the workspace are released together with the same version. Pushing a tag
`vX.Y.Z` runs `.github/workflows/release.yml`: CI, then `cargo publish --workspace`
(crates.io trusted publishing), then the GitHub release with generated notes.

The release is one PR on a topic branch (`release-X.Y.Z`). Claude pushes the branch and opens
the PR; the user merges it and pushes the tag.

## Checklist

1. **Review `[Unreleased]` in `CHANGELOG.md`.** Read it through. Compare the public API with the
   previous release and check that every removed or changed item is under a Breaking/Changed
   entry (see "Public API diff" below). Pick the version: before 1.0, a breaking change bumps
   the minor version.
2. **Bump the version** in the root `Cargo.toml`: `workspace.package.version` and the three
   `workspace.dependencies` entries (`cuprite_wire`, `cuprite_wire_kicad_lib`,
   `cuprite_wire_macros`). Then `cargo update --workspace` to update `Cargo.lock`.
3. **Regenerate the goldens**, which contain `(tool "cuprite_wire X.Y.Z")`:
   ```bash
   UPDATE_GOLDEN=1 cargo test --workspace
   UPDATE_GOLDEN=1 CUPRITE_VENDOR_MODELS_DIR=/workspace/vendor_models.local cargo test --workspace --all-features
   ```
   Check that `git diff tests/golden` changes only the version.
4. **Cut the CHANGELOG**: rename `[Unreleased]` to `[X.Y.Z] - YYYY-MM-DD`, add an empty
   `[Unreleased]` above it, and update the comparison links at the bottom
   (`[Unreleased]: .../compare/vX.Y.Z...HEAD`, `[X.Y.Z]: .../compare/vPREV...vX.Y.Z`).
5. **Check the packages** (see "`cargo package`" below) and the docs.rs rendering:
   `RUSTDOCFLAGS="--cfg docsrs" cargo +nightly doc -p cuprite_wire --features shared-ngspice --no-deps`
   (the settings of `[package.metadata.docs.rs]`; nightly is in the devcontainer).
6. **Run the usual checks**: `cargo fmt --check`, `cargo clippy --workspace --all-targets
   --all-features`, `cargo test --workspace --all-features`.
7. **Open the PR.** After the user merges it, update local `main` and create the tag locally:
   `git tag vX.Y.Z <merge commit>` (a lightweight tag on the merge commit, like the earlier
   ones). The user pushes it
   (`git push origin vX.Y.Z`).
8. **Verify**: the Release workflow succeeded, all crates show the new version on crates.io, the
   GitHub release exists, and docs.rs built the docs.

## Public API diff

`cargo public-api` is on PATH. Do not use its `diff` subcommand (it checks out commits in the
working tree). Instead list both versions and diff the files:

```bash
mkdir -p $TMPDIR/prev && git archive vPREV | tar -x -C $TMPDIR/prev
(cd $TMPDIR/prev && cargo public-api -p cuprite_wire --simplified) > $TMPDIR/api-prev.txt
cargo public-api -p cuprite_wire --simplified > $TMPDIR/api-new.txt
diff $TMPDIR/api-prev.txt $TMPDIR/api-new.txt
```

Use no features for both, and repeat for the other public crates as needed
(`cuprite_wire_kicad_lib` is exempt from semver; see its crate docs).

## Gotchas

- **`cargo package`**: in the sandbox, the untracked `.gitmodules` at the repository root cannot
  be read and packaging fails. Package a clean copy instead (needs the network, about a minute):
  ```bash
  rm -rf $TMPDIR/pkg && mkdir -p $TMPDIR/pkg && git archive HEAD | tar -x -C $TMPDIR/pkg
  cd $TMPDIR/pkg && git init -q && git add -A && git commit -qm pkg && cargo package --workspace
  ```
- **Tag push**: a tag ruleset restricts tag creation, and the required status checks rejected
  the first push. The user pushes the tag.
- **A crate never published before**: crates.io trusted publishing cannot do the first publish
  (403), so `cargo publish --workspace` fails midway and the GitHub release job does not run.
  Have the user publish the new crate manually first (`cargo publish -p <crate>` with their API
  token) and configure its trusted publisher on crates.io; if needed, create the release by hand:
  `gh release create vX.Y.Z --verify-tag --generate-notes`.
- **Workflow changes**: commits that change `.github/workflows/` cannot be pushed with Claude's
  token (no workflow scope). Commit on a separate branch and ask the user to push and open the
  PR. Give `gh pr create` an explicit `--title` / `--body` (not `--fill`), so the body has no
  commit trailers.
- **Old toolchains** (e.g. checking the MSRV):
  `RUSTUP_HOME=$TMPDIR/rustup rustup toolchain install <ver> --profile minimal`
  (allowed domain: `static.rust-lang.org`).
