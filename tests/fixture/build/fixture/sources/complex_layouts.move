// Complex struct layout tests
module fixture::complex_layouts;

// Nested structs
public struct Inner has drop {
    value: u64
}

public struct Outer has drop {
    inner: Inner,
    count: u64
}

public struct NestedStruct has drop {
    outer: Outer
}

// Generic structs
public struct Container<T> has drop {
    value: T
}

public struct Pair<T, U> has drop {
    first: T,
    second: U
}

// Vectors of structs
public struct VectorWrapper has drop {
    items: vector<u64>
}

// Test functions
public fun test_nested_struct(): NestedStruct {
    NestedStruct {
        outer: Outer {
            inner: Inner { value: 42 },
            count: 10
        }
    }
}

public fun test_generic_u64(): Container<u64> {
    Container { value: 42 }
}

public fun test_generic_wrapper(): Container<Inner> {
    Container { value: Inner { value: 100 } }
}

public fun test_vector_wrapper(): VectorWrapper {
    VectorWrapper { items: vector<u64>[] }
}
