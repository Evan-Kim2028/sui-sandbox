// Trigger A4: Object parameter detection
module fixture::a4_object_param;

public struct Coin has key {
    value: u64
}

// Function takes an object parameter (mutable reference)
public fun transfer_coin(coin: &mut Coin): u64 {
    coin.value
}

// Test should detect this as having object params and skip Tier B
