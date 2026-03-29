use mirokio::runtime::mirokio as Runtime;

#[test]
#[should_panic(expected = "runtime requires at least one worker thread")]
fn test_runtime_zero_workers_panics_or_errors() {
    let _ = Runtime::new(0);
}
