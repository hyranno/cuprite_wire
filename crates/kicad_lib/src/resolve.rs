//! Locating library files, reading them with a cache, and recording what was read so that
//! the macro can make the compiler track it.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

use crate::{
    Error,
    footprint::Footprint,
    spice_lib::{self, SpiceLib},
    symbol::SymbolLib,
};

pub const SYMBOL_DIR_ENV: &str = "KICAD9_SYMBOL_DIR";
pub const FOOTPRINT_DIR_ENV: &str = "KICAD9_FOOTPRINT_DIR";
pub const DEFAULT_SYMBOL_DIR: &str = "/usr/share/kicad/symbols";
pub const DEFAULT_FOOTPRINT_DIR: &str = "/usr/share/kicad/footprints";

/// Where libraries are looked up, and what has been read.
pub struct Resolver {
    /// Base of relative paths (the crate using the macro).
    pub manifest_dir: PathBuf,
    /// Overrides of the symbol / footprint directories (relative to `manifest_dir`).
    pub symbol_dir: Option<PathBuf>,
    pub footprint_dir: Option<PathBuf>,
    /// Base of relative `Sim.Library` paths (relative to `manifest_dir`).
    pub sim_library_dir: Option<PathBuf>,
    env: Box<dyn Fn(&str) -> Option<String>>,
    /// Files read, for rebuild tracking.
    pub files: Vec<PathBuf>,
    /// Environment variables consulted, for rebuild tracking.
    pub env_vars: Vec<String>,
}

impl Resolver {
    pub fn new(
        manifest_dir: impl Into<PathBuf>,
        env: impl Fn(&str) -> Option<String> + 'static,
    ) -> Self {
        Resolver {
            manifest_dir: manifest_dir.into(),
            symbol_dir: None,
            footprint_dir: None,
            sim_library_dir: None,
            env: Box::new(env),
            files: Vec::new(),
            env_vars: Vec::new(),
        }
    }

    fn var(&mut self, name: &str) -> Option<String> {
        if !self.env_vars.iter().any(|v| v == name) {
            self.env_vars.push(name.to_string());
        }
        (self.env)(name).filter(|value| !value.is_empty())
    }

    fn dir(&mut self, overridden: Option<PathBuf>, env: &str, default: &str) -> PathBuf {
        match overridden {
            Some(dir) => self.manifest_dir.join(dir),
            None => self
                .var(env)
                .map_or_else(|| PathBuf::from(default), PathBuf::from),
        }
    }

    pub fn symbol_dir(&mut self) -> PathBuf {
        self.dir(self.symbol_dir.clone(), SYMBOL_DIR_ENV, DEFAULT_SYMBOL_DIR)
    }

    pub fn footprint_dir(&mut self) -> PathBuf {
        self.dir(
            self.footprint_dir.clone(),
            FOOTPRINT_DIR_ENV,
            DEFAULT_FOOTPRINT_DIR,
        )
    }

    pub fn symbol_lib(&mut self, library: &str) -> Result<Arc<SymbolLib>, Error> {
        let path = self.symbol_dir().join(format!("{library}.kicad_sym"));
        if !path.is_file() {
            self.track(&path);
            return Err(Error::new(format!(
                "symbol library {library:?} not found ({})",
                path.display()
            )));
        }
        self.load(&path, utf8, SymbolLib::parse)
    }

    pub fn footprint(&mut self, library: &str, name: &str) -> Result<Arc<Footprint>, Error> {
        let path = self
            .footprint_dir()
            .join(format!("{library}.pretty"))
            .join(format!("{name}.kicad_mod"));
        if !path.is_file() {
            self.track(&path);
            return Err(Error::new(format!(
                "footprint {library}:{name} not found ({})",
                path.display()
            )));
        }
        self.load(&path, utf8, Footprint::parse)
    }

    pub fn spice_lib(&mut self, path: &Path) -> Result<Arc<SpiceLib>, Error> {
        self.load(path, |bytes| Ok(spice_lib::decode(bytes)), SpiceLib::parse)
    }

    /// Resolves a `Sim.Library` value: expands `${VAR}` and makes relative paths absolute.
    pub fn sim_library_path(&mut self, value: &str) -> Result<PathBuf, Error> {
        let mut expanded = String::new();
        let mut rest = value;
        while let Some(start) = rest.find("${") {
            expanded.push_str(&rest[..start]);
            let end = rest[start..]
                .find('}')
                .ok_or_else(|| Error::new(format!("unterminated variable in {value:?}")))?;
            let name = &rest[start + 2..start + end];
            let replacement = if is_symbol_dir_var(name) {
                self.symbol_dir().to_string_lossy().into_owned()
            } else {
                self.var(name).ok_or_else(|| {
                    Error::new(format!(
                        "environment variable {name} in {value:?} is not set"
                    ))
                })?
            };
            expanded.push_str(&replacement);
            rest = &rest[start + end + 1..];
        }
        expanded.push_str(rest);
        let path = PathBuf::from(expanded);
        Ok(if path.is_absolute() {
            path
        } else {
            let base = match &self.sim_library_dir {
                Some(dir) => self.manifest_dir.join(dir),
                None => self.manifest_dir.clone(),
            };
            base.join(path)
        })
    }

    fn load<T: Send + Sync + 'static>(
        &mut self,
        path: &Path,
        decode: fn(Vec<u8>) -> Result<String, Error>,
        parse: impl FnOnce(&str) -> Result<T, Error>,
    ) -> Result<Arc<T>, Error> {
        let path = self.track(path);
        cached(&path, decode, parse).map_err(|e| e.context(path.display()))
    }

    fn track(&mut self, path: &Path) -> PathBuf {
        let path = normalize(path);
        if !self.files.contains(&path) {
            self.files.push(path.clone());
        }
        path
    }
}

/// KiCad files are UTF-8.
fn utf8(bytes: Vec<u8>) -> Result<String, Error> {
    String::from_utf8(bytes).map_err(|_| Error::new("the file is not valid UTF-8"))
}

/// `KICAD<n>_SYMBOL_DIR` of any version names the symbol directory (older projects
/// and the KiCad demos use `KICAD7_SYMBOL_DIR`).
fn is_symbol_dir_var(name: &str) -> bool {
    name.strip_prefix("KICAD")
        .and_then(|rest| rest.strip_suffix("_SYMBOL_DIR"))
        .is_some_and(|version| !version.is_empty() && version.chars().all(|c| c.is_ascii_digit()))
}

/// Removes `.` components (no symlink resolution, so the path stays as the user wrote it).
fn normalize(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

type Entry = (SystemTime, u64, Arc<dyn std::any::Any + Send + Sync>);

/// Parses each file once per process (the macro runs many times in one compiler process).
/// An entry is reused only while the file's modification time and size are unchanged, since
/// the process may outlive edits to the file (e.g. in rust-analyzer).
fn cached<T: Send + Sync + 'static>(
    path: &Path,
    decode: fn(Vec<u8>) -> Result<String, Error>,
    parse: impl FnOnce(&str) -> Result<T, Error>,
) -> Result<Arc<T>, Error> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Entry>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let metadata = std::fs::metadata(path).map_err(|e| Error::new(e.to_string()))?;
    let stamp = (
        metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        metadata.len(),
    );
    if let Some((modified, len, value)) = cache.lock().unwrap().get(path)
        && (*modified, *len) == stamp
        && let Ok(value) = value.clone().downcast::<T>()
    {
        return Ok(value);
    }
    let bytes = std::fs::read(path).map_err(|e| Error::new(e.to_string()))?;
    let value = Arc::new(parse(&decode(bytes)?)?);
    cache
        .lock()
        .unwrap()
        .insert(path.to_path_buf(), (stamp.0, stamp.1, value.clone()));
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sim_library_path() {
        let mut resolver = Resolver::new("/crate", |name| {
            (name == "MODELS").then(|| "/models".to_string())
        });
        assert_eq!(
            resolver
                .sim_library_path("${KICAD7_SYMBOL_DIR}/Simulation_SPICE.sp")
                .unwrap(),
            PathBuf::from("/usr/share/kicad/symbols/Simulation_SPICE.sp")
        );
        assert_eq!(
            resolver.sim_library_path("${MODELS}/d.lib").unwrap(),
            PathBuf::from("/models/d.lib")
        );
        assert_eq!(
            resolver.sim_library_path("m/d.lib").unwrap(),
            PathBuf::from("/crate/m/d.lib")
        );
        resolver.sim_library_dir = Some("spice".into());
        assert_eq!(
            resolver.sim_library_path("d.lib").unwrap(),
            PathBuf::from("/crate/spice/d.lib")
        );
        assert!(resolver.sim_library_path("${NONE}/d.lib").is_err());
        assert_eq!(resolver.env_vars, ["KICAD9_SYMBOL_DIR", "MODELS", "NONE"]);
    }

    #[test]
    fn test_dirs() {
        let mut resolver = Resolver::new("/crate", |_| None);
        assert_eq!(resolver.symbol_dir(), PathBuf::from(DEFAULT_SYMBOL_DIR));
        resolver.footprint_dir = Some("libs/fp".into());
        assert_eq!(resolver.footprint_dir(), PathBuf::from("/crate/libs/fp"));
        let mut resolver = Resolver::new("/crate", |_| Some("/sym".into()));
        assert_eq!(resolver.symbol_dir(), PathBuf::from("/sym"));
    }
}
