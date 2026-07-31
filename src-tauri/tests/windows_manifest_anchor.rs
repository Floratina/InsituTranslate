// Keep an explicit test target so Cargo accepts the test-only resource link
// emitted by `embed-resource::compile_for_tests` during `cargo check --lib`.
#[test]
fn test_target_is_present() {}
