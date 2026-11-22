//! Building Verilog designs with Verilator into libraries for `d_cosim`, as ngspice's `vlnggen`
//! script does: the design is converted to C++, its ports are read from the generated class, and
//! it is compiled with ngspice's `verilator_shim.cpp` into a shared library.

use std::{
    ffi::OsString,
    fmt, fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use super::DCosim;
use crate::spice::{
    digital::{InputCharacteristics, OutputCharacteristics},
    simulation::output_log,
    subcircuit::{check_subcircuit_pin, fnv1a},
    temp_dir::TempDir,
};

/// Where ngspice installs `verilator_shim.cpp`, `verilator_main.cpp` and the headers they include.
pub const DEFAULT_SHIM_DIR: &str = "/usr/share/ngspice/scripts/src";

/// The prefix of the generated C++ class, which `verilator_shim.cpp` expects.
const PREFIX: &str = "Vlng";

/// The widest port the shim handles (it shifts `int`s).
const MAX_WIDTH: u32 = 32;

/// Added to the errors of a missing program or shim: outside Linux, these are most likely because
/// the build is not supported there.
const SUPPORT_NOTE: &str = if cfg!(target_os = "linux") {
    ""
} else {
    " (Verilator builds are supported on Linux only; on Windows, use WSL 2 or the devcontainer)"
};

/// A Verilator build of Verilog sources, cached by the contents of the sources, the arguments and
/// the version of Verilator.
///
/// ```no_run
/// use cuprite_wire::spice::{digital::*, xspice::Verilator};
///
/// let model = Verilator::new(["/abs/path/counter.v"]).build()?;
/// let part = model.part(&InputCharacteristics::cmos(3.3), &OutputCharacteristics::cmos(3.3));
/// # Ok::<(), cuprite_wire::spice::xspice::VerilatorError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verilator {
    sources: Vec<PathBuf>,
    args: Vec<String>,
    program: PathBuf,
    shim_dir: PathBuf,
    cache_dir: PathBuf,
}

impl Verilator {
    /// The Verilog sources of the design (the top module is found by Verilator, or given with
    /// `--top-module` in [`Verilator::arg`]). The cache is in `cuprite_wire/verilator` in the
    /// user's cache directory: `$XDG_CACHE_HOME`, else `$HOME/.cache` (`%LOCALAPPDATA%` on
    /// Windows), else the system temporary directory.
    pub fn new<P: Into<PathBuf>>(sources: impl IntoIterator<Item = P>) -> Self {
        Verilator {
            sources: sources.into_iter().map(Into::into).collect(),
            args: Vec::new(),
            program: "verilator".into(),
            shim_dir: DEFAULT_SHIM_DIR.into(),
            cache_dir: cache_home(|name| std::env::var_os(name))
                .unwrap_or_else(std::env::temp_dir)
                .join(env!("CARGO_PKG_NAME"))
                .join("verilator"),
        }
    }

    /// An argument for Verilator, e.g. `--top-module`, `-GWIDTH=8` or `-Wno-fatal`.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// The Verilator program (`verilator` from `PATH` by default).
    pub fn program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    /// The directory of `verilator_shim.cpp` ([`DEFAULT_SHIM_DIR`] by default).
    pub fn shim_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.shim_dir = dir.into();
        self
    }

    /// The directory the builds are cached in. The libraries built there are loaded into the
    /// process, so on Unix it must belong to the user and not be writable by others; it is
    /// created with mode `0700`.
    pub fn cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = dir.into();
        self
    }

    /// Builds the design, or takes it from the cache.
    ///
    /// Relative paths (the sources, the shim and cache directories, and the paths in the
    /// arguments) are resolved against the current directory, which is part of the cache key.
    ///
    /// # Errors
    /// [`VerilatorError::Command`] if a step of the build fails, [`VerilatorError::Ports`] if the
    /// ports of the top module cannot be used, and [`VerilatorError::Io`] if a file operation
    /// fails, a program cannot be run, or the cache directory belongs to another user or is
    /// writable by others.
    pub fn build(&self) -> Result<VerilatedModel, VerilatorError> {
        let this = self.absolute()?;
        create_private_dir(&this.cache_dir)?;
        let dir = this.cache_dir.join(format!("{:016x}", this.key()?));
        let library = dir.join("libmodel.so");
        if !library.exists() || !inputs_unchanged(&dir) {
            let staging = TempDir::new_in(&this.cache_dir, "build")?;
            match this.build_in(&staging) {
                Ok(()) => {}
                Err(e @ VerilatorError::Command { .. }) => {
                    // Kept for inspection; the error names it.
                    staging.keep();
                    return Err(e);
                }
                Err(e) => return Err(e),
            }
            install(staging, &dir)?;
        }
        let header = fs::read_to_string(dir.join("obj").join(format!("{PREFIX}.h")))?;
        let (inputs, outputs) = read_ports(&header)?;
        Ok(VerilatedModel {
            library,
            inputs,
            outputs,
        })
    }

    /// The builder with absolute paths. The commands run in the current directory, but `make`
    /// runs in the object directory, so the shim directory must be absolute, and [`DCosim::new`]
    /// takes an absolute library.
    fn absolute(&self) -> io::Result<Self> {
        Ok(Verilator {
            sources: self
                .sources
                .iter()
                .map(std::path::absolute)
                .collect::<io::Result<_>>()?,
            args: self.args.clone(),
            program: self.program.clone(),
            shim_dir: std::path::absolute(&self.shim_dir)?,
            cache_dir: std::path::absolute(&self.cache_dir)?,
        })
    }

    /// The cache key: the version of Verilator, the shim, the sources, the arguments and the
    /// current directory (which relative paths in the arguments are resolved against). Files that
    /// the sources and arguments read in (`` `include ``, `-y`, `-f`) are checked against the
    /// record of the build ([`inputs_unchanged`]).
    fn key(&self) -> Result<u64, VerilatorError> {
        let cwd = std::env::current_dir()?;
        let mut text: Vec<u8> = Vec::new();
        let shims = [
            self.shim_dir.join("verilator_shim.cpp"),
            self.shim_dir.join("verilator_main.cpp"),
        ];
        for (i, file) in shims.iter().chain(&self.sources).enumerate() {
            text.push(0);
            text.extend_from_slice(file.display().to_string().as_bytes());
            text.push(0);
            text.extend_from_slice(&fs::read(file).map_err(|e| {
                let hint = if i < shims.len() && e.kind() == io::ErrorKind::NotFound {
                    format!(
                        "; set `shim_dir` to the `share/ngspice/scripts/src` directory of \
                        ngspice{SUPPORT_NOTE}"
                    )
                } else {
                    String::new()
                };
                io::Error::new(
                    e.kind(),
                    format!("cannot read {}: {e}{hint}", file.display()),
                )
            })?);
        }
        text.push(0);
        text.extend_from_slice(
            self.run(Command::new(&self.program).arg("--version"), None)?
                .as_bytes(),
        );
        for arg in &self.args {
            text.push(0);
            text.extend_from_slice(arg.as_bytes());
        }
        text.push(0);
        text.extend_from_slice(cwd.display().to_string().as_bytes());
        Ok(fnv1a(&text))
    }

    /// The steps of `vlnggen`, building in `dir`.
    fn build_in(&self, dir: &Path) -> Result<(), VerilatorError> {
        let obj = dir.join("obj");
        let verilator = || {
            let mut command = Command::new(&self.program);
            command
                .arg("--Mdir")
                .arg(&obj)
                .args(["--prefix", PREFIX, "--CFLAGS", "-fpic", "--cc"]);
            command
        };
        let mut convert = verilator();
        convert.args(&self.args).args(&self.sources);
        self.run(&mut convert, Some(dir))?;

        let header = fs::read_to_string(obj.join(format!("{PREFIX}.h")))?;
        let (inputs, outputs) = read_ports(&header)?;
        let macros = |ports: &[Port]| {
            let mut text = String::from("/* Generated code: do not edit. */\n");
            for port in ports {
                text.push_str(&port.macro_line);
                text.push('\n');
            }
            text
        };
        fs::write(obj.join("inputs.h"), macros(&inputs))?;
        fs::write(obj.join("outputs.h"), macros(&outputs))?;
        fs::write(obj.join("inouts.h"), macros(&[]))?;

        let mut compile = verilator();
        compile
            .arg("--CFLAGS")
            .arg(format!("-I{}", self.shim_dir.display()))
            .args(["--build", "--exe", "-j", "0"])
            .arg(self.shim_dir.join("verilator_main.cpp"))
            .arg(self.shim_dir.join("verilator_shim.cpp"))
            .args(&self.args)
            .args(&self.sources);
        self.run(&mut compile, Some(dir))?;

        let mut link = Command::new("g++");
        link.arg("--shared")
            .args(["verilator_shim.o", "verilated.o", "verilated_threads.o"].map(|o| obj.join(o)))
            .arg(obj.join(format!("{PREFIX}__ALL.a")))
            .args(["-pthread", "-lpthread", "-o"])
            .arg(dir.join("libmodel.so"));
        self.run(&mut link, Some(dir))?;

        let files = fs::read_to_string(obj.join(format!("{PREFIX}__verFiles.dat")))?;
        fs::write(dir.join(INPUTS), record_inputs(&files)?)?;
        Ok(())
    }

    /// Runs a command in the current directory; its output, or an error with it that names `dir`
    /// (the build directory), if any.
    fn run(&self, command: &mut Command, dir: Option<&Path>) -> Result<String, VerilatorError> {
        let output = command.output().map_err(|e| {
            let note = if e.kind() == io::ErrorKind::NotFound {
                SUPPORT_NOTE
            } else {
                ""
            };
            io::Error::new(
                e.kind(),
                format!(
                    "cannot run {}: {e}{note}",
                    command.get_program().to_string_lossy()
                ),
            )
        })?;
        let log = output_log(&output);
        if output.status.success() {
            Ok(log)
        } else {
            Err(VerilatorError::Command {
                command: format!("{command:?}"),
                dir: dir.map(Path::to_path_buf),
                log,
            })
        }
    }
}

/// The user's cache directory from the environment variables (`var`): `$XDG_CACHE_HOME`, else
/// `$HOME/.cache` (`%LOCALAPPDATA%` on Windows). Relative paths are ignored, as the XDG Base
/// Directory Specification says.
fn cache_home(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name| var(name).map(PathBuf::from).filter(|p| p.is_absolute());
    if cfg!(windows) {
        absolute("LOCALAPPDATA")
    } else {
        absolute("XDG_CACHE_HOME").or_else(|| absolute("HOME").map(|home| home.join(".cache")))
    }
}

/// Creates the cache directory `dir` (mode `0700` on Unix) if it does not exist, and checks that
/// other users cannot replace the libraries in it: on Unix, it must not be writable by the group
/// or others, and (on Linux) must belong to the user.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};

        builder.mode(0o700);
        builder.create(dir)?;
        let metadata = fs::metadata(dir)?;
        // `/proc/self` belongs to the effective user of the process.
        let user = fs::metadata("/proc/self").map(|m| m.uid()).ok();
        let problem = if user.is_some_and(|uid| uid != metadata.uid()) {
            "belongs to another user"
        } else if metadata.mode() & 0o022 != 0 {
            "is writable by the group or others"
        } else {
            return Ok(());
        };
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "the cache directory {} {problem}, who could replace the libraries loaded from \
                it; set `cache_dir` to a directory of your own",
                dir.display()
            ),
        ))
    }
    #[cfg(not(unix))]
    builder.create(dir)
}

/// Moves the build in `staging` to `dir`, replacing an outdated build there. Another build of
/// the same key may have been installed first; either is fine.
///
/// # Errors
/// If the build cannot be moved and `dir` has no up-to-date build either (e.g. the outdated
/// one cannot be removed).
fn install(staging: TempDir, dir: &Path) -> io::Result<()> {
    if dir.exists() && !inputs_unchanged(dir) {
        // A library already loaded keeps working after it is removed.
        let _ = fs::remove_dir_all(dir);
    }
    match fs::rename(&*staging, dir) {
        Ok(()) => {
            staging.keep();
            Ok(())
        }
        Err(_) if dir.join("libmodel.so").exists() && inputs_unchanged(dir) => Ok(()),
        Err(e) => Err(io::Error::new(
            e.kind(),
            format!("cannot move the build to {}: {e}", dir.display()),
        )),
    }
}

/// The file of a build that records the files Verilator read: `<size> <mtime s> <mtime ns>
/// <absolute path>` per line.
const INPUTS: &str = "inputs";

/// The record of the inputs ([`INPUTS`]) from Verilator's `<prefix>__verFiles.dat`, whose lines
/// `S <size> <ino> <ctime s> <ctime ns> <mtime s> <mtime ns> "<path>"` are the files it read
/// (the sources, the files they include, Verilator itself), with paths relative to the current
/// directory.
fn record_inputs(files: &str) -> io::Result<String> {
    let mut record = String::new();
    for line in files.lines() {
        let Some(rest) = line.strip_prefix("S ") else {
            continue;
        };
        let parsed = rest.split_once('"').and_then(|(stat, path)| {
            let stat: Vec<&str> = stat.split_whitespace().collect();
            let path = path.strip_suffix('"')?;
            (stat.len() == 6 && !path.contains('\n')).then(|| (stat[0], stat[4], stat[5], path))
        });
        let Some((size, secs, nanos, path)) = parsed else {
            return Err(io::Error::other(format!(
                "cannot read the line {line:?} of {PREFIX}__verFiles.dat"
            )));
        };
        let path = std::path::absolute(path)?;
        record.push_str(&format!("{size} {secs} {nanos} {}\n", path.display()));
    }
    Ok(record)
}

/// Whether the files read by the build in `dir` still have the size and modification time they
/// had ([`INPUTS`]). A build without the record is outdated.
fn inputs_unchanged(dir: &Path) -> bool {
    let Ok(record) = fs::read_to_string(dir.join(INPUTS)) else {
        return false;
    };
    record.lines().all(|line| {
        let mut fields = line.splitn(4, ' ');
        let (Some(size), Some(secs), Some(nanos), Some(path)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return false;
        };
        let Ok(metadata) = fs::metadata(path) else {
            return false;
        };
        let Some(modified) = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        else {
            return false;
        };
        size == metadata.len().to_string()
            && secs == modified.as_secs().to_string()
            && nanos == modified.subsec_nanos().to_string()
    })
}

/// A port of the top module.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Port {
    /// Name of the port in the top module.
    pub name: String,
    /// Index of the most significant bit (`7` of `[7:0]`).
    pub msb: u32,
    /// Index of the least significant bit (`0` of `[7:0]`).
    pub lsb: u32,
    /// The line for `inputs.h` / `outputs.h`: `VL_DATA(8,clk,0,0)`.
    macro_line: String,
}

impl Port {
    /// The number of bits.
    pub fn width(&self) -> u32 {
        self.msb - self.lsb + 1
    }

    /// The pins of the port's bits in the order of the `d_cosim` ports: the name for a single
    /// bit, else `<name>_<bit>` from the most significant bit.
    pub fn pins(&self) -> Vec<String> {
        if self.width() == 1 && self.lsb == 0 {
            vec![self.name.clone()]
        } else {
            (self.lsb..=self.msb)
                .rev()
                .map(|bit| format!("{}_{bit}", self.name))
                .collect()
        }
    }
}

/// The input and output ports declared in the generated class (`VL_IN8(&clk,0,0);`), in order,
/// with pins that can be added to a [`DCosim`].
fn read_ports(header: &str) -> Result<(Vec<Port>, Vec<Port>), VerilatorError> {
    let (mut inputs, mut outputs) = (Vec::new(), Vec::new());
    for line in header.lines() {
        let line = line.trim();
        let (list, rest) = if let Some(rest) = line.strip_prefix("VL_INOUT") {
            let name = rest.split(['&', ',']).nth(1).unwrap_or(rest);
            return Err(VerilatorError::Ports(format!(
                "the inout port {name:?} is not supported"
            )));
        } else if let Some(rest) = line.strip_prefix("VL_IN") {
            (&mut inputs, rest)
        } else if let Some(rest) = line.strip_prefix("VL_OUT") {
            (&mut outputs, rest)
        } else {
            continue;
        };
        let parse = || -> Option<Port> {
            let (size, rest) = rest.split_once("(&")?;
            let args = rest.split_once(')')?.0;
            let mut fields = args.split(',');
            let name = fields.next()?.trim().to_string();
            let msb: u32 = fields.next()?.trim().parse().ok()?;
            let lsb: u32 = fields.next()?.trim().parse().ok()?;
            Some(Port {
                macro_line: format!("VL_DATA({size},{name},{msb},{lsb})"),
                name,
                msb,
                lsb,
            })
        };
        let port = parse().ok_or_else(|| {
            VerilatorError::Ports(format!("cannot read the port declaration {line:?}"))
        })?;
        if port.msb < port.lsb || port.width() > MAX_WIDTH {
            return Err(VerilatorError::Ports(format!(
                "the port {:?} [{}:{}] is not supported (ports of 1 to {MAX_WIDTH} bits, msb >= lsb)",
                port.name, port.msb, port.lsb
            )));
        }
        list.push(port);
    }
    let mut pins: Vec<String> = Vec::new();
    for pin in inputs.iter().chain(&outputs).flat_map(Port::pins) {
        check_subcircuit_pin(&pin, pins.iter().map(String::as_str), "the design")
            .map_err(VerilatorError::Ports)?;
        pins.push(pin);
    }
    Ok((inputs, outputs))
}

/// A design built by [`Verilator::build`]: the library and the ports of its top module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerilatedModel {
    library: PathBuf,
    inputs: Vec<Port>,
    outputs: Vec<Port>,
}

impl VerilatedModel {
    /// The absolute path of the shared library, the `simulation` parameter of `d_cosim`.
    pub fn library(&self) -> &Path {
        &self.library
    }

    /// The input ports, in the order of the `d_cosim` inputs.
    pub fn inputs(&self) -> &[Port] {
        &self.inputs
    }

    /// The output ports, in the order of the `d_cosim` outputs.
    pub fn outputs(&self) -> &[Port] {
        &self.outputs
    }

    /// A [`DCosim`] part of the design, with a pin per bit ([`Port::pins`]), all inputs with
    /// `input` and all outputs with `output`. For pins with other characteristics, add the pins
    /// to `DCosim::new(model.library())` in the same order instead.
    pub fn part(&self, input: &InputCharacteristics, output: &OutputCharacteristics) -> DCosim {
        let mut part = DCosim::new(&self.library);
        for pin in self.inputs.iter().flat_map(Port::pins) {
            part = part.input(&pin, input.clone());
        }
        for pin in self.outputs.iter().flat_map(Port::pins) {
            part = part.output(&pin, output.clone());
        }
        part
    }
}

/// An error building a design with Verilator.
#[derive(Debug)]
#[non_exhaustive]
pub enum VerilatorError {
    /// A file operation failed, or a program of the build cannot be run.
    Io(io::Error),
    /// A step of the build failed. Its build directory is kept; other errors remove it.
    Command {
        /// The command line that failed.
        command: String,
        /// The build directory kept for inspection, or `None` if the step was not part of a
        /// build (`verilator --version`).
        dir: Option<PathBuf>,
        /// The output of the command.
        log: String,
    },
    /// The ports of the top module cannot be used.
    Ports(String),
}

impl fmt::Display for VerilatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerilatorError::Io(e) => write!(f, "cannot build with Verilator: {e}"),
            VerilatorError::Command { command, dir, log } => {
                write!(f, "a step of the Verilator build failed")?;
                if let Some(dir) = dir {
                    write!(f, " (in {})", dir.display())?;
                }
                write!(f, ": {command}\n{log}")
            }
            VerilatorError::Ports(reason) => write!(f, "cannot use the design: {reason}"),
        }
    }
}

impl std::error::Error for VerilatorError {}

impl From<io::Error> for VerilatorError {
    fn from(e: io::Error) -> Self {
        VerilatorError::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_ports() {
        let header = "class Vlng {\n  public:\n    VL_IN8(&clk,0,0);\n    VL_OUT16(&q,11,4);\n    VL_IN(&w,31,0);\n};\n";
        let (inputs, outputs) = read_ports(header).unwrap();
        let pins: Vec<String> = inputs.iter().flat_map(Port::pins).collect();
        assert_eq!(pins.len(), 33);
        assert_eq!(pins[..3], ["clk", "w_31", "w_30"]);
        assert_eq!(pins[32], "w_0");
        assert_eq!(inputs[0].macro_line, "VL_DATA(8,clk,0,0)");
        assert_eq!(inputs[1].macro_line, "VL_DATA(,w,31,0)");
        assert_eq!(
            outputs[0].pins(),
            ["q_11", "q_10", "q_9", "q_8", "q_7", "q_6", "q_5", "q_4"]
        );
        assert!(matches!(
            read_ports("VL_INOUT8(&sda,0,0);"),
            Err(VerilatorError::Ports(reason)) if reason.contains("\"sda\"")
        ));
        assert!(matches!(
            read_ports("VL_IN64(&x,63,0);"),
            Err(VerilatorError::Ports(_))
        ));
    }

    /// Ports whose pins cannot be added to the part are reported, not left to panic in
    /// [`VerilatedModel::part`].
    #[test]
    fn test_read_ports_pin_names() {
        let error = |header| match read_ports(header) {
            Err(VerilatorError::Ports(reason)) => reason,
            other => panic!("{other:?}"),
        };
        assert!(error("VL_IN8(&GND,0,0);").contains("\"GND\""));
        assert!(error("VL_IN8(&ref,0,0);").contains("reserved"));
        assert!(error("VL_IN8(&q_0,0,0);\nVL_OUT8(&q,1,0);").contains("already has a pin \"q_0\""));
    }

    #[test]
    fn test_cache_home() {
        let home = |vars: &[(&str, &str)]| {
            let vars: Vec<(String, OsString)> = vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.into()))
                .collect();
            cache_home(|name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
        };
        if cfg!(windows) {
            assert_eq!(home(&[("LOCALAPPDATA", r"C:\x")]), Some(r"C:\x".into()));
            assert_eq!(home(&[("HOME", r"C:\h")]), None);
        } else {
            assert_eq!(
                home(&[("XDG_CACHE_HOME", "/c"), ("HOME", "/h")]),
                Some("/c".into())
            );
            assert_eq!(
                home(&[("XDG_CACHE_HOME", "c"), ("HOME", "/h")]),
                Some("/h/.cache".into())
            );
            assert_eq!(home(&[("HOME", "")]), None);
            assert_eq!(home(&[]), None);
        }
    }

    /// The cache directory is created private, and one that others can write to is rejected.
    #[test]
    #[cfg(unix)]
    fn test_create_private_dir() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new("verilator-private").unwrap();
        let dir = temp.join("a/cache");
        create_private_dir(&dir).unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        create_private_dir(&dir).unwrap();

        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        let error = create_private_dir(&dir).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            error
                .to_string()
                .contains("writable by the group or others"),
            "{error}"
        );
    }

    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "Verilator builds are supported on Linux only"
    )]
    fn test_record_inputs() {
        let files = "# DESCRIPTION: ...\nC \"--cc top.v\"\n\
            S       108  3116647  1795221207   251409447  1795221207   251409447 \"sp ace/top.v\"\n\
            T       580  3116757  1795221207   300954585  1795221207   300954585 \"obj/Vlng__ver.d\"\n\
            S  13920736  3102769  1795185144   381464455  1743199713           0 \"/usr/bin/verilator_bin\"\n";
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            record_inputs(files).unwrap(),
            format!(
                "108 1795221207 251409447 {}/sp ace/top.v\n13920736 1743199713 0 /usr/bin/verilator_bin\n",
                cwd.display()
            )
        );
        assert!(record_inputs("S 1 2 \"x\"\n").is_err());
    }
}
