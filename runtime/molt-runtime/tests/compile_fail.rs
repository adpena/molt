/// Compile-fail tests verify that unsafe memory patterns are rejected at compile time.
/// Inspired by Monty's heap_reader_compile_fail_cases.
#[test]
fn compile_fail_tests() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail_cases/*.rs");
}
