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

    /// Get gross turnover (deposited + withdrawn)
    public fun gross_turnover(account: &TradingAccount): u64 {
        account.total_deposited + account.total_withdrawn
    }

    /// Whether account has a positive net position
    public fun is_net_positive(account: &TradingAccount): bool {
        account.total_deposited > account.total_withdrawn
    }

    /// Compact account summary:
    /// (total_deposited, total_withdrawn, net_position, gross_turnover, is_net_positive)
    public fun account_summary(account: &TradingAccount): (u64, u64, u64, u64, bool) {
        let deposited = account.total_deposited;
        let withdrawn = account.total_withdrawn;
        let net = if (deposited > withdrawn) {
            deposited - withdrawn
        } else {
            0
        };
        let turnover = deposited + withdrawn;
        let positive = deposited > withdrawn;
        (deposited, withdrawn, net, turnover, positive)
    }
}
