// Trigger A5: Generic function (not supported yet)
module fixture::a5_generic_function;

public struct Container<T> has drop {
    value: T
}

// Generic function with type parameter
public fun generic_function<T: drop>(value: T): Container<T> {
    Container { value }
}

// Test should fail at A5 because generic functions are not supported
