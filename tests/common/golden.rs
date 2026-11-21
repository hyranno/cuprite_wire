//! Golden files, shared by the unit tests (`src/test_parts.rs`) and the integration tests.

use std::path::Path;

/// Compares `actual` with `tests/golden/<name>`, with the crate directory written as
/// `${CARGO_MANIFEST_DIR}`. Run the tests with `UPDATE_GOLDEN=1` to (re)generate the files.
pub fn check_golden(name: &str, actual: &str) {
    let actual = actual.replace(env!("CARGO_MANIFEST_DIR"), "${CARGO_MANIFEST_DIR}");
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing; run with UPDATE_GOLDEN=1", path.display()));
    assert_eq!(actual, expected, "output differs from {}", path.display());
}
