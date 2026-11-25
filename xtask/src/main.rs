//! Development tasks of the workspace, run as `cargo xtask <TASK>...` (the alias is in
//! `.cargo/config.toml`).
//!
//! The checks of CI are defined here only: the CI workflow, the git hooks in `.githooks/` and
//! `CONTRIBUTING.md` run these tasks.

use std::env;
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

/// The optional features the checks build with (`kicad-libraries` only enables tests).
const FEATURES: &str = "ibis,shared-ngspice";

const USAGE: &str = "\
Usage: cargo xtask <TASK>... [OPTIONS]

Tasks:
  fmt         Check the formatting
  lint        Clippy and rustdoc warnings, with and without the optional features
  check       cargo check with the optional features (CI runs it with the minimum supported Rust)
  deny        Check the licenses and sources of the dependencies (cargo-deny)
  advisories  Check the dependencies for security advisories (cargo-deny; needs the network)
  test        Run the tests, including those reading the KiCad libraries and demos (and
              clippy on the targets needing the KiCad libraries)
  all         fmt, lint, deny and test: the checks of the pre-push hook

Options:
  --no-kicad        test: skip the tests (and clippy) reading the KiCad libraries and demos
  --no-kicad-demos  test: skip the tests reading the KiCad demos (Debian's packages lack them)
";

#[derive(Clone, Copy)]
enum Task {
    Fmt,
    Lint,
    Check,
    Deny,
    Advisories,
    Test,
}

/// The tests reading the KiCad installation that `test` runs.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum KicadTests {
    None,
    Libraries,
    LibrariesAndDemos,
}

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut tasks = Vec::new();
    let mut kicad = KicadTests::LibrariesAndDemos;
    for arg in args {
        match arg.as_str() {
            "help" | "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "fmt" => tasks.push(Task::Fmt),
            "lint" => tasks.push(Task::Lint),
            "check" => tasks.push(Task::Check),
            "deny" => tasks.push(Task::Deny),
            "advisories" => tasks.push(Task::Advisories),
            "test" => tasks.push(Task::Test),
            "all" => tasks.extend([Task::Fmt, Task::Lint, Task::Deny, Task::Test]),
            "--no-kicad" => kicad = KicadTests::None,
            "--no-kicad-demos" => kicad = kicad.min(KicadTests::Libraries),
            _ => return Err(format!("unknown argument `{arg}`\n\n{USAGE}")),
        }
    }
    if tasks.is_empty() {
        return Err(format!("no task given\n\n{USAGE}"));
    }
    for task in tasks {
        match task {
            Task::Fmt => cargo("fmt --all --check")?,
            Task::Lint => lint()?,
            Task::Check => cargo(&format!(
                "check --workspace --all-targets --features {FEATURES}"
            ))?,
            Task::Deny => deny("licenses bans sources")?,
            Task::Advisories => deny("advisories")?,
            Task::Test => test(kicad)?,
        }
    }
    Ok(())
}

fn lint() -> Result<(), String> {
    cargo(&format!(
        "clippy --workspace --all-targets --features {FEATURES} -- -D warnings"
    ))?;
    // The run above has the optional features; check cuprite_wire without them too.
    cargo("clippy -p cuprite_wire --all-targets -- -D warnings")?;
    // Broken intra-doc links and other rustdoc warnings, with and without the optional features
    // (links to items behind a feature break without it).
    let deny_warnings = [("RUSTDOCFLAGS", "-D warnings")];
    cargo_with_env(
        &format!("doc --workspace --no-deps --features {FEATURES}"),
        &deny_warnings,
    )?;
    cargo_with_env("doc -p cuprite_wire --no-deps", &deny_warnings)
}

/// Runs `cargo deny check` (configured in `deny.toml`).
fn deny(checks: &str) -> Result<(), String> {
    let installed = Command::new(cargo_program())
        .args(["deny", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !installed {
        return Err(
            "cargo-deny is not installed (the devcontainer has it): cargo install --locked cargo-deny"
                .into(),
        );
    }
    // All the members of the workspace; otherwise only cuprite_wire and its dependencies.
    cargo(&format!("deny --workspace check {checks}"))
}

fn test(kicad: KicadTests) -> Result<(), String> {
    let features = if kicad == KicadTests::None {
        FEATURES.to_string()
    } else {
        format!("{FEATURES},kicad-libraries")
    };
    if kicad != KicadTests::None {
        // The targets needing the KiCad libraries build only where they are installed, so `lint`
        // leaves them out.
        cargo(&format!(
            "clippy --workspace --all-targets --features {features} -- -D warnings"
        ))?;
    }
    cargo(&format!("test --workspace --features {features}"))?;
    // Ignored tests reading the KiCad symbol and footprint libraries, and the IBIS files of the
    // KiCad demos.
    match kicad {
        KicadTests::None => Ok(()),
        KicadTests::Libraries => cargo("test -p cuprite_wire_kicad_lib -- --ignored"),
        KicadTests::LibrariesAndDemos => {
            cargo("test -p cuprite_wire_kicad_lib -p cuprite_wire_ibis -- --ignored")
        }
    }
}

fn cargo(command: &str) -> Result<(), String> {
    cargo_with_env(command, &[])
}

/// Runs cargo with the arguments in `command` (separated by spaces) in the workspace root, and
/// fails if it fails.
fn cargo_with_env(command: &str, envs: &[(&str, &str)]) -> Result<(), String> {
    let assignments: String = envs
        .iter()
        .map(|(key, value)| format!("{key}='{value}' "))
        .collect();
    let shown = format!("{assignments}cargo {command}");
    eprintln!("$ {shown}");
    let status = Command::new(cargo_program())
        .args(command.split_whitespace())
        .envs(envs.iter().copied())
        .current_dir(workspace_root())
        .status()
        .map_err(|error| format!("cannot run cargo: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("failed: {shown}"))
    }
}

/// The cargo running xtask, so that the tasks use its toolchain (e.g. `cargo +1.89 xtask check`).
fn cargo_program() -> OsString {
    env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask is a directory of the workspace root")
}
