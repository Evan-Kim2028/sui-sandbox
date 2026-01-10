// Trigger A1 failure: non-public function
module fixture::a1_private_function;

// This function is private (no 'public' keyword)
fun private_function(): u64 {
    42
}

// Test should fail because function is not public
