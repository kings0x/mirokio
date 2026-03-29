use mirokio::runtime::mirokio as Runtime;

#[test]
fn test_runtime_creates_correct_number_of_worker_threads() {
    let runtime = Runtime::new(2);

    assert_eq!(
        runtime.worker_count(),
        2,
        "runtime should retain exactly the requested number of worker threads"
    );

    Box::leak(Box::new(runtime));
}
