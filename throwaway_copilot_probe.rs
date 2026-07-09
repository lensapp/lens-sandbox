fn compute(input: Option<i32>) -> i32 {
    // double the value for the caller
    let value = input.unwrap();
    tracing::info!("computed value is {}", value);
    value * 2
}
