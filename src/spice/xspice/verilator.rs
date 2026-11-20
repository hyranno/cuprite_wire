//! Building Verilog designs with Verilator into libraries for `d_cosim`, as ngspice's `vlnggen`
//! script does: the design is converted to C++, its ports are read from the generated class, and
//! it is compiled with ngspice's `verilator_shim.cpp` into a shared library.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::DCosim;
use crate::spice::digital::{InputCharacteristics, OutputCharacteristics, fnv1a};

/// Where ngspice installs `verilator_shim.cpp`, `verilator_main.cpp` and the headers they include.
pub const DEFAULT_SHIM_DIR: &str = "/usr/share/ngspice/scripts/src";

/// The prefix of the generated C++ class, which `verilator_shim.cpp` expects.
const PREFIX: &str = "Vlng";

/// The widest port the shim handles (it shifts `int`s).
const MAX_WIDTH: u32 = 32;

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
    /// `--top-module` in [`Verilator::arg`]). The cache is in `cuprite_wire-verilator` in the
    /// system temporary directory.
    pub fn new<P: Into<PathBuf>>(sources: impl IntoIterator<Item = P>) -> Self {
        Verilator {
            sources: sources.into_iter().map(Into::into).collect(),
            args: Vec::new(),
            program: "verilator".into(),
            shim_dir: DEFAULT_SHIM_DIR.into(),
            cache_dir: std::env::temp_dir().join(format!("{}-verilator", env!("CARGO_PKG_NAME"))),
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

    /// The directory the builds are cached in.
    pub fn cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = dir.into();
        self
    }

    /// Builds the design, or takes it from the cache.
    pub fn build(&self) -> Result<VerilatedModel, VerilatorError> {
        let dir = self.cache_dir.join(format!("{:016x}", self.key()?));
        let library = dir.join("libmodel.so");
        if !library.exists() {
            fs::create_dir_all(&self.cache_dir)?;
            let staging = staging_dir(&self.cache_dir)?;
            // On failure the staging directory is kept for inspection; the error names it.
            self.build_in(&staging)?;
            // Another build of the same key may have finished first; either is fine.
            if fs::rename(&staging, &dir).is_err() {
                let _ = fs::remove_dir_all(&staging);
            }
        }
        let header = fs::read_to_string(dir.join("obj").join(format!("{PREFIX}.h")))?;
        let (inputs, outputs) = read_ports(&header)?;
        Ok(VerilatedModel {
            library,
            inputs,
            outputs,
        })
    }

    /// The cache key: the version of Verilator, the shim, the sources and the arguments.
    fn key(&self) -> Result<u64, VerilatorError> {
        let version = self.run(Command::new(&self.program).arg("--version"), Path::new("."))?;
        let mut text = version;
        for file in [
            self.shim_dir.join("verilator_shim.cpp"),
            self.shim_dir.join("verilator_main.cpp"),
        ]
        .iter()
        .chain(&self.sources)
        {
            text.push('\0');
            text.push_str(&file.display().to_string());
            text.push('\0');
            text.push_str(&fs::read_to_string(file).map_err(|e| {
                io::Error::new(e.kind(), format!("cannot read {}: {e}", file.display()))
            })?);
        }
        for arg in &self.args {
            text.push('\0');
            text.push_str(arg);
        }
        Ok(fnv1a(text.as_bytes()))
    }

    /// The steps of `vlnggen`, in `dir`.
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
        self.run(&mut convert, dir)?;

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
        self.run(&mut compile, dir)?;

        let mut link = Command::new("g++");
        link.arg("--shared")
            .args(["verilator_shim.o", "verilated.o", "verilated_threads.o"].map(|o| obj.join(o)))
            .arg(obj.join(format!("{PREFIX}__ALL.a")))
            .args(["-pthread", "-lpthread", "-o"])
            .arg(dir.join("libmodel.so"));
        self.run(&mut link, dir)?;
        Ok(())
    }

    /// Runs a command in `dir`; its output, or an error with it.
    fn run(&self, command: &mut Command, dir: &Path) -> Result<String, VerilatorError> {
        command.current_dir(dir);
        let output = command.output().map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "cannot run {}: {e}",
                    command.get_program().to_string_lossy()
                ),
            )
        })?;
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if output.status.success() {
            Ok(log)
        } else {
            Err(VerilatorError::Command {
                command: format!("{command:?}"),
                dir: dir.to_path_buf(),
                log,
            })
        }
    }
}

/// A new directory to build in, renamed to its key when the build succeeds.
fn staging_dir(parent: &Path) -> io::Result<PathBuf> {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    loop {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = parent.join(format!("build-{}-{n}", std::process::id()));
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

/// A port of the top module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub name: String,
    pub msb: u32,
    pub lsb: u32,
    /// The line for `inputs.h` / `outputs.h`: `VL_DATA(8,clk,0,0)`.
    macro_line: String,
}

impl Port {
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

/// The input and output ports declared in the generated class (`VL_IN8(&clk,0,0);`), in order.
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
    pub fn library(&self) -> &Path {
        &self.library
    }

    pub fn inputs(&self) -> &[Port] {
        &self.inputs
    }

    pub fn outputs(&self) -> &[Port] {
        &self.outputs
    }

    /// A [`DCosim`] part of the design, with a pin per bit ([`Port::pins`]), all inputs with
    /// `input` and all outputs with `output`. For pins with other characteristics, add the pins
    /// to `DCosim::new(model.library())` in the same order instead.
    pub fn part(
        &self,
        input: &InputCharacteristics,
        output: &OutputCharacteristics,
    ) -> Box<DCosim> {
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

#[derive(Debug)]
pub enum VerilatorError {
    Io(io::Error),
    /// A step of the build failed. Its directory is kept.
    Command {
        command: String,
        dir: PathBuf,
        log: String,
    },
    /// The ports of the top module cannot be used.
    Ports(String),
}

impl fmt::Display for VerilatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerilatorError::Io(e) => write!(f, "cannot build with Verilator: {e}"),
            VerilatorError::Command { command, dir, log } => write!(
                f,
                "a step of the Verilator build failed (in {}): {command}\n{log}",
                dir.display()
            ),
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
}
