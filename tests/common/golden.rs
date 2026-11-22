//! Golden files.

use std::path::Path;

/// Compares `actual` with `tests/golden/<name>`, with the crate directory written as
/// `${CARGO_MANIFEST_DIR}` (SPICE netlists write it with `/` on Windows). Run the tests with
/// `UPDATE_GOLDEN=1` to (re)generate the files.
pub fn check_golden(name: &str, actual: &str) {
    let dir = env!("CARGO_MANIFEST_DIR");
    let actual = actual
        .replace(dir, "${CARGO_MANIFEST_DIR}")
        .replace(&dir.replace('\\', "/"), "${CARGO_MANIFEST_DIR}");
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
