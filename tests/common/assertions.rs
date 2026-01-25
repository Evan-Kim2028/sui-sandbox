//! Custom assertion utilities for tests.
//!
//! Provides assertion helpers that give better error messages and
//! standardize common assertion patterns.

/// Assert that a result is Ok and return the inner value.
///
/// Provides a better error message than `.unwrap()` by including context.
///
/// # Arguments
///
/// * `result` - The Result to check
/// * `context` - Description of what operation was being performed
///
/// # Returns
///
/// The `Ok` value if successful.
///
/// # Panics
///
/// Panics with a descriptive message if the result is `Err`.
#[allow(dead_code)]
pub fn assert_ok<T, E: std::fmt::Debug>(result: Result<T, E>, context: &str) -> T {
    match result {
        Ok(v) => v,
        Err(e) => panic!("{} failed: {:?}", context, e),
    }
}

/// Assert that a result is Err.
///
/// # Arguments
///
/// * `result` - The Result to check
/// * `context` - Description of what operation was being performed
///
/// # Panics
///
/// Panics if the result is `Ok`.
#[allow(dead_code)]
pub fn assert_err<T: std::fmt::Debug, E: std::fmt::Debug>(result: Result<T, E>, context: &str) {
    if let Ok(v) = result {
        panic!("{} should have failed but got: {:?}", context, v);
    }
}

/// Assert that an error message contains expected text.
///
/// This is useful for verifying that error messages are actionable and
/// contain relevant context.
///
/// # Arguments
///
/// * `error` - The error to check
/// * `expected_text` - Text that should appear in the error message
/// * `context` - Description of what the error is about
///
/// # Panics
///
/// Panics if the error message doesn't contain the expected text.
#[allow(dead_code)]
pub fn assert_error_contains<E: std::fmt::Display>(error: E, expected_text: &str, context: &str) {
    let error_str = error.to_string().to_lowercase();
    let expected_lower = expected_text.to_lowercase();

    assert!(
        error_str.contains(&expected_lower),
        "{}: error message should contain '{}', got: {}",
        context,
        expected_text,
        error
    );
}

/// Assert that an error message contains any of the expected texts.
///
/// # Arguments
///
/// * `error` - The error to check
/// * `expected_texts` - List of possible texts that should appear
/// * `context` - Description of what the error is about
#[allow(dead_code)]
pub fn assert_error_contains_any<E: std::fmt::Display>(
    error: E,
    expected_texts: &[&str],
    context: &str,
) {
    let error_str = error.to_string().to_lowercase();

    let found = expected_texts
        .iter()
        .any(|text| error_str.contains(&text.to_lowercase()));

    assert!(
        found,
        "{}: error message should contain one of {:?}, got: {}",
        context, expected_texts, error
    );
}

/// Assert that a value is within an expected range.
///
/// # Arguments
///
/// * `value` - The value to check
/// * `min` - Minimum expected value (inclusive)
/// * `max` - Maximum expected value (inclusive)
/// * `context` - Description of what value is being checked
#[allow(dead_code)]
pub fn assert_in_range<T: PartialOrd + std::fmt::Debug>(value: T, min: T, max: T, context: &str) {
    assert!(
        value >= min && value <= max,
        "{}: expected value in range [{:?}, {:?}], got {:?}",
        context,
        min,
        max,
        value
    );
}

/// Assert that a collection is not empty.
#[allow(dead_code)]
pub fn assert_not_empty<T>(collection: &[T], context: &str) {
    assert!(
        !collection.is_empty(),
        "{}: expected non-empty collection",
        context
    );
}

/// Assert that two byte slices are equal with better error output.
#[allow(dead_code)]
pub fn assert_bytes_eq(actual: &[u8], expected: &[u8], context: &str) {
    if actual != expected {
        panic!(
            "{}: byte mismatch\n  expected len: {}\n  actual len: {}\n  expected: {:?}\n  actual: {:?}",
            context,
            expected.len(),
            actual.len(),
            expected,
            actual
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assert_ok() {
        let result: Result<i32, &str> = Ok(42);
        let value = assert_ok(result, "test operation");
        assert_eq!(value, 42);
    }

    #[test]
    #[should_panic(expected = "test operation failed")]
    fn test_assert_ok_fails() {
        let result: Result<i32, &str> = Err("error");
        assert_ok(result, "test operation");
    }

    #[test]
    fn test_assert_err() {
        let result: Result<i32, &str> = Err("error");
        assert_err(result, "test operation");
    }

    #[test]
    fn test_assert_error_contains() {
        let error = "module not found: test_module";
        assert_error_contains(error, "not found", "module lookup");
    }

    #[test]
    fn test_assert_error_contains_any() {
        let error = "function not found in module";
        assert_error_contains_any(error, &["not found", "missing", "error"], "function lookup");
    }

    #[test]
    fn test_assert_in_range() {
        assert_in_range(5, 1, 10, "value check");
        assert_in_range(1, 1, 10, "lower bound");
        assert_in_range(10, 1, 10, "upper bound");
    }

    #[test]
    fn test_assert_not_empty() {
        let items = vec![1, 2, 3];
        assert_not_empty(&items, "items list");
    }
}
