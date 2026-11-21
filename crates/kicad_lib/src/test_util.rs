//! Helpers shared by the unit tests.

use crate::resolve::Resolver;

/// A resolver for the files under `tests/fixtures`, without environment variables.
pub fn fixture_resolver() -> Resolver {
    Resolver::new(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"),
        |_| None,
    )
}

pub fn strings(s: &[&str]) -> Vec<String> {
    s.iter().map(|s| s.to_string()).collect()
}

pub fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
    p.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}
