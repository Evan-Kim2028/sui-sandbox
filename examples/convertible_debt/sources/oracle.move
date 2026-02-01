module convertible_debt::oracle {
    use sui::object;
    use sui::object::UID;
    use sui::transfer;
    use sui::tx_context::{Self, TxContext};

    /// Price scale for ETH/USD (1e6 = 6 decimals)
    const PRICE_SCALE: u64 = 1_000_000;

    const E_NOT_ADMIN: u64 = 0;

    /// Simple shared oracle for demo pricing.
    public struct Oracle has key {
        id: UID,
        admin: address,
        price: u64,
        last_update_ms: u64,
    }

    /// Create and share an oracle with an initial price.
    public fun create_shared(initial_price: u64, ctx: &mut TxContext) {
        let oracle = Oracle {
            id: object::new(ctx),
            admin: tx_context::sender(ctx),
            price: initial_price,
            last_update_ms: 0,
        };
        transfer::share_object(oracle);
    }

    /// Update price (admin only).
    public fun set_price(oracle: &mut Oracle, new_price: u64, now_ms: u64, ctx: &TxContext) {
        assert!(tx_context::sender(ctx) == oracle.admin, E_NOT_ADMIN);
        oracle.price = new_price;
        oracle.last_update_ms = now_ms;
    }

    public fun price_scale(): u64 {
        PRICE_SCALE
    }

    public fun get_price(oracle: &Oracle): u64 {
        oracle.price
    }
}
