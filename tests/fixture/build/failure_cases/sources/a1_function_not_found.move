// Trigger A1 failure: function not found
module fixture::a1_function_not_found;

// This module has a public function
public fun valid_function(): u64 {
    42
}

// The test will try to call "nonexistent_function" which doesn't exist
