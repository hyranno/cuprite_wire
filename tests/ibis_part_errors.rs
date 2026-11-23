//! Compile errors of `ibis_part!`, checked with trybuild (`tests/ui/ibis/*.stderr`).
//! Run with `TRYBUILD=overwrite` to (re)generate the expected output.

#[test]
fn test_compile_errors() {
    // SAFETY: this is the only test in this binary, so no other thread reads the environment.
    // The test crates are built elsewhere, so the file is given as `${TEST_MODELS}/...`.
    unsafe {
        std::env::set_var(
            "TEST_MODELS",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/crates/kicad_lib/tests/fixtures/models"
            ),
        );
    }
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/ibis/*.rs");
}
