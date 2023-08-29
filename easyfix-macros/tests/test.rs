#[test]
fn test() {
    let t = trybuild::TestCases::new();
    t.pass("tests/scenarios/compile_ok.rs");
    t.compile_fail("tests/scenarios/fail_on_multibyte_character.rs");
    t.compile_fail("tests/scenarios/fail_on_control_character.rs");
}
