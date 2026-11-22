//! Reading a SPICE library with the files it includes, and the definitions a model uses.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    Error,
    resolve::Resolver,
    spice_lib::{Definition, SpiceLib, strip_comment},
};

/// A library file, the section that is read (`None`: outside the sections) and its path.
pub(super) type LibFile = (Arc<SpiceLib>, Option<String>, PathBuf);

/// The most files a library may read with its includes, to stop include loops through paths that
/// keep growing (`.include sub/self.lib` with `sub` a link to `.`).
pub(super) const MAX_LIBRARY_FILES: usize = 256;

/// Reads a library and the files it includes, each with the section that is read.
pub(super) fn read_library(path: &Path, resolver: &mut Resolver) -> Result<Vec<LibFile>, Error> {
    let mut libs = Vec::new();
    let mut queue = vec![(path.to_path_buf(), None::<String>)];
    let mut seen = BTreeSet::new();
    while let Some((file, section)) = queue.pop() {
        let key = (
            file.clone(),
            section.as_ref().map(|s| s.to_ascii_lowercase()),
        );
        if !seen.insert(key) {
            continue;
        }
        if seen.len() > MAX_LIBRARY_FILES {
            return Err(Error::new(format!(
                "{} includes more than {MAX_LIBRARY_FILES} files; do the includes loop?",
                path.display()
            )));
        }
        let lib = resolver.spice_lib(&file)?;
        if let Some(section) = &section
            && !lib.sections.iter().any(|s| s.eq_ignore_ascii_case(section))
        {
            return Err(Error::new(format!(
                "{} has no section {section:?}",
                file.display()
            )));
        }
        let dir = file.parent().map(PathBuf::from).unwrap_or_default();
        // Relative paths are relative to the including file, as in ngspice.
        let mut includes: Vec<_> = lib
            .includes(section.as_deref())
            .map(|include| (dir.join(&include.path), include.section.clone()))
            .collect();
        includes.reverse();
        queue.extend(includes);
        libs.push((lib, section, file));
    }
    Ok(libs)
}

/// The definition (in `file`) and the top-level definitions (also `.param` / `.func`) it refers
/// to by name, each with the file it is in.
pub(super) fn dependencies(
    definition: &Definition,
    file: &Path,
    libs: &[LibFile],
) -> Vec<(Definition, PathBuf)> {
    let find_any = |name: &str| {
        libs.iter().find_map(|(lib, section, path)| {
            lib.definitions(section.as_deref())
                .find(|d| d.name.eq_ignore_ascii_case(name))
                .map(|d| (d.clone(), path.clone()))
        })
    };
    let mut out: Vec<(Definition, PathBuf)> = Vec::new();
    let mut pending = vec![(definition.clone(), file.to_path_buf())];
    while let Some((def, path)) = pending.pop() {
        if out
            .iter()
            .any(|(d, _)| d.name.eq_ignore_ascii_case(&def.name))
        {
            continue;
        }
        for token in referenced_names(&def.text) {
            if let Some(referenced) = find_any(token) {
                pending.push(referenced);
            }
        }
        out.push((def, path));
    }
    out
}

/// Names that a definition may refer to: the words of its text after the keyword and its own name.
/// Names may contain `+` and `-` (`IN+`), which are also operators, so both forms are returned.
pub(super) fn referenced_names(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('*'))
        .map(strip_comment)
        .flat_map(|line| {
            line.split(|c: char| c.is_whitespace() || "(),={}'\"*/<>&|?:!^;".contains(c))
        })
        .filter(|word| !word.is_empty())
        .skip(2)
        .flat_map(|word| {
            std::iter::once(word).chain(word.split(['+', '-']).filter(|w| !w.is_empty()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_referenced_names_skip_comments() {
        let names: Vec<&str> =
            referenced_names(".subckt s a b\nx1 a b inner ; other\n* third\n+ $ fourth\n.ends")
                .collect();
        assert!(names.contains(&"inner"), "{names:?}");
        for comment in ["other", "third", "fourth"] {
            assert!(!names.contains(&comment), "{names:?}");
        }
    }
}
