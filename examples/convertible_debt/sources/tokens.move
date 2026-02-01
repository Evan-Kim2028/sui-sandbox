module convertible_debt::tokens {
    /// USD stablecoin marker type (6 decimals, off-chain semantics)
    public struct USD has drop {}
    /// ETH-like collateral marker type (9 decimals, off-chain semantics)
    public struct ETH has drop {}
}
