//! Error messages that help to fix a binding: similar names, pin tables and suggested mappings.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use super::{
    PartPin,
    library::{LibFile, referenced_names},
};
use crate::{
    Error,
    spice_lib::{Definition, DefinitionKind},
};

/// The error for a name that is not in the library, with candidates.
pub(super) fn not_found(name: &str, path: &Path, libs: &[LibFile]) -> Error {
    let definitions: Vec<&Definition> = libs
        .iter()
        .flat_map(|(lib, section, _)| lib.definitions(section.as_deref()))
        .filter(|d| {
            matches!(
                d.kind,
                DefinitionKind::Subckt { .. } | DefinitionKind::Model { .. }
            )
        })
        .collect();
    let mut message = format!("no .subckt or .model named {name:?} in {}", path.display());
    let similar = similar_names(name, definitions.iter().map(|d| d.name.as_str()));
    if !similar.is_empty() {
        message.push_str(&format!("; similar names: {}", similar.join(", ")));
    }
    // Subcircuits no other definition uses are likely the products of the library.
    let used: BTreeSet<String> = definitions
        .iter()
        .flat_map(|d| {
            referenced_names(&d.text)
                .filter(|name| !name.eq_ignore_ascii_case(&d.name))
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        })
        .collect();
    let top: Vec<&str> = definitions
        .iter()
        .filter(|d| {
            matches!(d.kind, DefinitionKind::Subckt { .. })
                && !used.contains(&d.name.to_ascii_lowercase())
        })
        .map(|d| d.name.as_str())
        .take(10)
        .collect();
    if !top.is_empty() {
        message.push_str(&format!(
            "; subcircuits not used by others: {}",
            top.join(", ")
        ));
    }
    let encrypted: usize = libs.iter().map(|(lib, _, _)| lib.encrypted_blocks).sum();
    if encrypted > 0 {
        message.push_str(&format!(
            "; the library has {encrypted} encrypted blocks, which may hide it"
        ));
    }
    let sections: Vec<&str> = libs
        .iter()
        .flat_map(|(lib, _, _)| lib.sections.iter().map(String::as_str))
        .collect();
    if !sections.is_empty() {
        message.push_str(&format!(
            "; definitions in `.lib` sections ({}) are found only through `.lib file section`",
            sections.join(", ")
        ));
    }
    Error::new(message)
}

pub(super) fn describe_pin(pin: &PartPin) -> String {
    match &pin.name {
        Some(name) if name != &pin.id => format!("{} ({name})", pin.id),
        _ => pin.id.clone(),
    }
}

/// A table of the pins and the ports they are mapped to (`ports_of` a pin ID), appended to an
/// error message.
pub(super) fn pin_table(pins: &[PartPin], ports_of: impl Fn(&str) -> Vec<String>) -> String {
    let mut text = String::from("\npins and ports:");
    for pin in pins {
        let ports = ports_of(&pin.id);
        let ports = if ports.is_empty() {
            "-".to_string()
        } else {
            ports.join(", ")
        };
        text.push_str(&format!("\n  {:<16} => {ports}", describe_pin(pin)));
    }
    text
}

/// Mappings suggested from the names, to copy into `pins` after checking them.
pub(super) fn suggestions(pins: &[&PartPin], ports: &[&str]) -> String {
    let mut by_name: BTreeMap<String, Vec<&PartPin>> = BTreeMap::new();
    for pin in pins {
        if let Some(name) = &pin.name {
            by_name.entry(normalize(name)).or_default().push(pin);
        }
    }
    let mut ports_by_name: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for port in ports {
        ports_by_name.entry(normalize(port)).or_default().push(port);
    }
    let mut lines = Vec::new();
    for pin in pins {
        let Some(key) = pin.name.as_deref().map(normalize) else {
            continue;
        };
        if let (Some([_]), Some([port])) = (
            by_name.get(&key).map(Vec::as_slice),
            ports_by_name.get(&key).map(Vec::as_slice),
        ) && !key.is_empty()
        {
            lines.push(format!("\n  {:?} => {:?},", pin.id, port));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "\nsuggested from the names (not applied; check them against the datasheet):{}",
        lines.concat()
    )
}

/// A name for matching pins and ports: uppercase, without `~{}/#` and a trailing `_N` / `_B` / `N` / `B`
/// (active-low markers); `+` and `-` are kept.
/// Up to 8 of `candidates` that look like `name`: one contains the other, or they differ in at
/// most 2 characters, after [`normalize`].
pub(crate) fn similar_names<'a>(
    name: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Vec<&'a str> {
    let wanted = normalize(name);
    candidates
        .into_iter()
        .filter(|candidate| {
            let candidate = normalize(candidate);
            candidate.contains(&wanted)
                || wanted.contains(&candidate)
                || edit_distance(&candidate, &wanted) <= 2
        })
        .take(8)
        .collect()
}

pub(super) fn normalize(name: &str) -> String {
    let upper: String = name
        .chars()
        .filter(|c| !"~{}/#".contains(*c))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    for suffix in ["_N", "_B", "N", "B"] {
        if let Some(stem) = upper.strip_suffix(suffix)
            && stem.len() >= 3
        {
            return stem.to_string();
        }
    }
    upper
}

pub(super) fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let current = row[j + 1];
            row[j + 1] = (previous + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(current + 1);
            previous = current;
        }
    }
    row[b.len()]
}
