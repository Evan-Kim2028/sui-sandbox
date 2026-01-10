module test_graph::test_graph {
    use sui::transfer;
    use sui::tx_context::TxContext;

    struct MyStruct has key, store {
        id: sui::object::UID,
    }

    public fun a(ctx: &mut TxContext) {
        b(ctx);
    }

    public fun b(ctx: &mut TxContext) {
        let s = MyStruct {
            id: sui::object::new(ctx),
        };
        transfer::public_transfer(s, @0x1);
    }
}
