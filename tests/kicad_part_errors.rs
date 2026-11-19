//! Compile errors of `kicad_part!` and `spice_part!`, checked with trybuild (`tests/ui/*.stderr`).
//! Run with `TRYBUILD=overwrite` to (re)generate the expected output.

#[test]
fn test_compile_errors() {
    let fixtures = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/kicad_lib/tests/fixtures"
    );
    // SAFETY: this is the only test in this binary, so no other thread reads the environment.
    // The cargo process run by trybuild inherits it, so the test cases use the test libraries.
    unsafe {
        std::env::set_var("KICAD9_SYMBOL_DIR", format!("{fixtures}/symbols"));
        std::env::set_var("KICAD9_FOOTPRINT_DIR", format!("{fixtures}/footprints"));
        // The test crates are built elsewhere, so model libraries are given as `${TEST_MODELS}/...`.
        std::env::set_var("TEST_MODELS", format!("{fixtures}/models"));
    }
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
