/// Balance Manager Helper
///
/// A simple wrapper that demonstrates interacting with forked DeepBook state.
/// This module provides utilities for managing trading balances.
module balance_helper::manager {
    /// A wrapper that tracks trading activity
    public struct TradingAccount has key, store {
        id: sui::object::UID,
        /// Total deposits made
        total_deposited: u64,
        /// Total withdrawals made
        total_withdrawn: u64,
    }

    /// Create a new trading account
    public fun new(ctx: &mut sui::tx_context::TxContext): TradingAccount {
        TradingAccount {
            id: sui::object::new(ctx),
            total_deposited: 0,
            total_withdrawn: 0,
        }
    }

    /// Record a deposit
    public fun record_deposit(account: &mut TradingAccount, amount: u64) {
        account.total_deposited = account.total_deposited + amount;
    }

    /// Record a withdrawal
    public fun record_withdrawal(account: &mut TradingAccount, amount: u64) {
        account.total_withdrawn = account.total_withdrawn + amount;
    }

    /// Get total deposited
    public fun total_deposited(account: &TradingAccount): u64 {
        account.total_deposited
    }

    /// Get total withdrawn
    public fun total_withdrawn(account: &TradingAccount): u64 {
        account.total_withdrawn
    }

    /// Get net position (deposited - withdrawn)
    public fun net_position(account: &TradingAccount): u64 {
        if (account.total_deposited > account.total_withdrawn) {
            account.total_deposited - account.total_withdrawn
        } else {
            0
        }
    }
}
