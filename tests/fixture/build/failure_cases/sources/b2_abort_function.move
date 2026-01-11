// Trigger B2: Execution abort
module fixture::b2_abort_function;

// This function always aborts
public fun always_abort(): u64 {
    abort 42
}

// Test should fail at B2 during VM execution
