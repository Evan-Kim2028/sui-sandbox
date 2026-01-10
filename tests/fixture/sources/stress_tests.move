module fixture::stress_tests {
    use sui::transfer;
    use sui::tx_context::{Self, TxContext};
    use sui::coin::{Self, TreasuryCap};

    public struct MyObj has key, store { id: sui::object::UID }
    public struct MyCoin has drop {}

    /// P0: Basic substitution
    public fun probe_coin<T>(c: coin::Coin<T>, ctx: &mut TxContext) {
        transfer::public_transfer(c, tx_context::sender(ctx));
    }

    /// P0: Nested substitution
    public fun probe_nested_cap<T>(cap: TreasuryCap<T>, ctx: &mut TxContext) {
        transfer::public_transfer(cap, tx_context::sender(ctx));
    }

    /// P1: Mutual Recursion (Loop Prevention)
    public fun recursive_a(ctx: &mut TxContext) {
        recursive_b(ctx);
    }

    fun recursive_b(ctx: &mut TxContext) {
        recursive_a(ctx);
    }

    /// P1: Depth Test (Object at Depth 2)
    public fun depth_0(ctx: &mut TxContext) {
        depth_1(ctx);
    }

    fun depth_1(ctx: &mut TxContext) {
        let obj = MyObj { id: sui::object::new(ctx) };
        transfer::public_transfer(obj, tx_context::sender(ctx));
    }
}
