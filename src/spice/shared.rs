//! Running decks in the ngspice shared library (libngspice), with the `shared-ngspice` feature.
//!
//! The library is loaded at run time, so building does not need it. It keeps global state, so it
//! is loaded once per process and runs one deck at a time: runs from several threads (the tests of
//! `cargo test`) wait for each other. A crash of ngspice takes the whole process down, which the
//! CLI ([`Ngspice`](crate::spice::Ngspice)) does not; prefer the CLI unless the run needs the
//! library (co-simulation with Rust models).

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    fs, io,
    path::{Path, PathBuf},
    ptr,
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

use libloading::Library;

use crate::spice::{
    SpiceDeck,
    simulation::{Simulation, SimulationError, Simulator, finish_run, prepare_run_dir},
};

/// The deck as the library runs it, next to [`DECK_FILE`](crate::spice::simulation::DECK_FILE).
const SHARED_DECK_FILE: &str = "shared.cir";

/// The file name of the library, found in the search path of the dynamic loader.
const DEFAULT_LIBRARY: &str = if cfg!(windows) {
    "ngspice.dll"
} else if cfg!(target_os = "macos") {
    "libngspice.0.dylib"
} else {
    "libngspice.so.0"
};

/// The ngspice shared library.
///
/// It runs the same deck as [`Ngspice`](crate::spice::Ngspice) and gives the same [`Simulation`]:
/// the run directory holds `deck.cir` (`ngspice -b deck.cir` there reproduces the run with the CLI)
/// and `shared.cir`, the form the library runs. That form has no `quit`, and the relative output
/// files of `wrdata`, `write` and `wrs2p` are made absolute, since the library runs in the working
/// directory of the process. Output files of other commands (e.g. `print ... > file`) must be
/// absolute. The compatibility mode is set with `set ngbehavior=...` before the deck is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedNgspice {
    /// The library file: a path, or a file name the dynamic loader searches for.
    pub library: PathBuf,
}

impl Default for SharedNgspice {
    /// `libngspice.so.0` (`libngspice.0.dylib` on macOS, `ngspice.dll` on Windows).
    fn default() -> Self {
        SharedNgspice {
            library: DEFAULT_LIBRARY.into(),
        }
    }
}

impl SharedNgspice {
    /// Runs the deck in a new temporary directory. Fails if ngspice reports an error.
    ///
    /// After an error ngspice cannot recover from, the library is unloaded and loaded again for
    /// the next run.
    pub fn run(&self, deck: &SpiceDeck) -> Result<Simulation, SimulationError> {
        let dir = prepare_run_dir(deck)?;
        // ngspice takes the rest of a command line as it is, quotes included.
        let deck_path = dir.join(SHARED_DECK_FILE);
        if deck_path
            .to_str()
            .is_none_or(|path| path.contains(|c: char| c.is_whitespace() || c == '"'))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the run directory {} cannot be written in a command of ngspice",
                    dir.display()
                ),
            )
            .into());
        }
        fs::write(&deck_path, deck.for_shared_library(&dir))?;

        let mut library = lock(&LIBRARY);
        if library.as_ref().is_none_or(|l| l.path != self.library) {
            // Unload the other library first: both would share the callbacks.
            *library = None;
            *library = Some(Loaded::load(&self.library)?);
        }
        let loaded = library.as_ref().expect("the library is loaded");
        lock(&OUTPUT).clear();
        EXITED.store(false, Ordering::SeqCst);
        let mode = match deck.netlist().ngbehavior() {
            Some(mode) => format!("set ngbehavior={mode}"),
            None => "unset ngbehavior".into(),
        };
        let succeeded = loaded.command(&mode)? == 0
            && loaded.command(&format!("source {}", deck_path.display()))? == 0;
        let log = std::mem::take(&mut *lock(&OUTPUT));
        let exited = EXITED.load(Ordering::SeqCst);
        if exited {
            // "ngspice.dll cannot recover and awaits to be reset or detached".
            *library = None;
        } else {
            // Free the circuit and its results.
            loaded.command("remcirc")?;
            loaded.command("destroy all")?;
            lock(&OUTPUT).clear();
        }
        drop(library);
        finish_run(dir, log, succeeded && !exited)
    }
}

impl Simulator for SharedNgspice {
    fn run(&self, deck: &SpiceDeck) -> Result<Simulation, SimulationError> {
        SharedNgspice::run(self, deck)
    }
}

/// The loaded library; `None` before the first run and after an error ngspice cannot recover from.
static LIBRARY: Mutex<Option<Loaded>> = Mutex::new(None);
/// The output of ngspice since it was last cleared, one line per call of [`send_char`].
static OUTPUT: Mutex<String> = Mutex::new(String::new());
/// Set by [`controlled_exit`].
static EXITED: AtomicBool = AtomicBool::new(false);

/// A run that panicked leaves nothing the next run relies on, so a poisoned lock is used as it is.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// The callbacks and functions of `sharedspice.h` used here.
type SendChar = unsafe extern "C" fn(*mut c_char, c_int, *mut c_void) -> c_int;
type SendStat = unsafe extern "C" fn(*mut c_char, c_int, *mut c_void) -> c_int;
type ControlledExit = unsafe extern "C" fn(c_int, bool, bool, c_int, *mut c_void) -> c_int;
type BgThreadRunning = unsafe extern "C" fn(bool, c_int, *mut c_void) -> c_int;
/// `ngSpice_Init(SendChar*, SendStat*, ControlledExit*, SendData*, SendInitData*,
/// BGThreadRunning*, void*)`; the vector callbacks are not used (null).
type NgSpiceInit = unsafe extern "C" fn(
    Option<SendChar>,
    Option<SendStat>,
    Option<ControlledExit>,
    *mut c_void,
    *mut c_void,
    Option<BgThreadRunning>,
    *mut c_void,
) -> c_int;
type NgSpiceCommand = unsafe extern "C" fn(*mut c_char) -> c_int;

struct Loaded {
    path: PathBuf,
    command: NgSpiceCommand,
    /// Keeps `command` valid; unloaded on drop.
    _library: Library,
}

impl Loaded {
    fn load(path: &Path) -> Result<Self, SimulationError> {
        let error = |what: String| SimulationError::SharedLibrary(what);
        // SAFETY: loading libngspice runs no initialization code that could break this process.
        let library = unsafe { Library::new(path.as_os_str()) }
            .map_err(|e| error(format!("cannot load {}: {e}", path.display())))?;
        // SAFETY: the types are those of `sharedspice.h` (ngspice 44).
        let (init, command) = unsafe {
            let init: NgSpiceInit = *library
                .get("ngSpice_Init")
                .map_err(|e| error(e.to_string()))?;
            let command: NgSpiceCommand = *library
                .get("ngSpice_Command")
                .map_err(|e| error(e.to_string()))?;
            (init, command)
        };
        // SAFETY: the callbacks have the types ngspice expects and live as long as the process.
        let status = unsafe {
            init(
                Some(send_char),
                Some(send_stat),
                Some(controlled_exit),
                ptr::null_mut(),
                ptr::null_mut(),
                Some(bg_thread_running),
                ptr::null_mut(),
            )
        };
        let banner = std::mem::take(&mut *lock(&OUTPUT));
        if status != 0 {
            return Err(error(format!("ngSpice_Init returned {status}\n{banner}")));
        }
        Ok(Loaded {
            path: path.to_path_buf(),
            command,
            _library: library,
        })
    }

    /// Runs a front-end command and returns the status (0 on success).
    fn command(&self, line: &str) -> Result<c_int, SimulationError> {
        let line = CString::new(line).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "a command of ngspice contains a NUL character",
            )
        })?;
        // ngspice may modify the line in place.
        let mut bytes = line.into_bytes_with_nul();
        // SAFETY: a NUL-terminated buffer that outlives the call; the library is loaded.
        Ok(unsafe { (self.command)(bytes.as_mut_ptr().cast()) })
    }
}

/// Collects a line of output, with the `stdout ` / `stderr ` prefix of the library removed.
unsafe extern "C" fn send_char(text: *mut c_char, _id: c_int, _user: *mut c_void) -> c_int {
    if text.is_null() {
        return 0;
    }
    // SAFETY: ngspice passes a NUL-terminated string valid during the call.
    let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    let line = ["stdout", "stderr"]
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
        .map_or(&*text, |rest| rest.strip_prefix(' ').unwrap_or(rest));
    let mut output = lock(&OUTPUT);
    output.push_str(line);
    output.push('\n');
    0
}

/// Progress messages (`tran 45%`) are not needed.
unsafe extern "C" fn send_stat(_text: *mut c_char, _id: c_int, _user: *mut c_void) -> c_int {
    0
}

/// ngspice wants to exit (`quit`, or an error it cannot recover from).
unsafe extern "C" fn controlled_exit(
    _status: c_int,
    _unload: bool,
    _quit: bool,
    _id: c_int,
    _user: *mut c_void,
) -> c_int {
    EXITED.store(true, Ordering::SeqCst);
    0
}

/// Runs are not started in the background thread (`bg_run`), so its state is not needed.
unsafe extern "C" fn bg_thread_running(_running: bool, _id: c_int, _user: *mut c_void) -> c_int {
    0
}
