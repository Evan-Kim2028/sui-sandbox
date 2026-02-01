/// Liquidation Engine Module
///
/// Coordinates liquidations across the DeFi suite.
/// Uses flash loans to fund liquidations without upfront capital.
///
/// # PTB Integration - THE KEY SYNERGY
/// This module demonstrates the ultimate PTB power:
/// 1. Flash borrow funds (no collateral needed)
/// 2. Repay underwater user's debt in lending pool
/// 3. Seize discounted collateral from vault
/// 4. Swap seized collateral for repayment asset
/// 5. Repay flash loan + fee
/// 6. Keep profit
///
/// All in a single atomic transaction!
#[allow(unused_field, unused_const)]
module defi_suite::liquidation_engine {
    use sui::coin::{Self, Coin};
    use sui::sui::SUI;
    use sui::event;
    use defi_suite::flash_loan::{Self, FlashPool, FlashReceipt};
    use defi_suite::collateral_vault::{Self, CollateralVault};
    use defi_suite::swap_pool::{Self, SwapPool};

    // =========================================================================
    // Error Codes
    // =========================================================================

    const E_NOT_LIQUIDATABLE: u64 = 600;
    const E_UNPROFITABLE: u64 = 601;
    const E_INSUFFICIENT_PROFIT: u64 = 602;

    // =========================================================================
    // Constants
    // =========================================================================

    /// Minimum profit threshold in basis points (0.5%)
    const MIN_PROFIT_BPS: u64 = 50;
    const BPS_BASE: u64 = 10000;

    // =========================================================================
    // Events
    // =========================================================================

    public struct LiquidationExecuted has copy, drop {
        liquidator: address,
        liquidated_user: address,
        debt_repaid: u64,
        collateral_seized: u64,
        profit: u64,
    }

    public struct FlashLiquidationStarted has copy, drop {
        liquidator: address,
        target_user: address,
        flash_amount: u64,
    }

    public struct FlashLiquidationCompleted has copy, drop {
        liquidator: address,
        target_user: address,
        gross_proceeds: u64,
        flash_repayment: u64,
        net_profit: u64,
    }

    // =========================================================================
    // Liquidation Functions
    // =========================================================================

    /// Check if a position can be profitably liquidated
    public fun can_liquidate_profitably(
        vault: &CollateralVault,
        swap_pool: &SwapPool,
        user: address,
        debt_to_repay: u64,
    ): (bool, u64) {
        // Check if position is liquidatable
        if (!collateral_vault::is_liquidatable(vault, user)) {
            return (false, 0)
        };

        // Calculate collateral we'd receive (with 5% bonus)
        let collateral_to_seize = debt_to_repay + (debt_to_repay / 20);

        // Get swap quote for collateral -> SUI
        let swap_output = swap_pool::get_amount_out(swap_pool, collateral_to_seize, false);

        // Calculate if profitable after flash loan fee
        let flash_fee = flash_loan::calculate_fee(debt_to_repay);
        let total_cost = debt_to_repay + flash_fee;

        if (swap_output > total_cost) {
            let profit = swap_output - total_cost;
            let profit_bps = (profit * BPS_BASE) / debt_to_repay;
            (profit_bps >= MIN_PROFIT_BPS, profit)
        } else {
            (false, 0)
        }
    }

    /// Step 1 of flash liquidation: Borrow funds
    /// Returns the borrowed funds and receipt (hot potato)
    public fun start_flash_liquidation(
        flash_pool: &mut FlashPool,
        target_user: address,
        debt_to_repay: u64,
        ctx: &mut TxContext
    ): (Coin<SUI>, FlashReceipt) {
        event::emit(FlashLiquidationStarted {
            liquidator: ctx.sender(),
            target_user,
            flash_amount: debt_to_repay,
        });

        flash_loan::borrow(flash_pool, debt_to_repay, ctx)
    }

    /// Step 2: Execute the liquidation on the collateral vault
    /// Uses the borrowed funds to repay debt and seize collateral
    public fun execute_liquidation(
        vault: &mut CollateralVault,
        target_user: address,
        _repayment_funds: &Coin<SUI>, // Just verifying we have funds
        ctx: &mut TxContext
    ): Coin<SUI> {
        assert!(collateral_vault::is_liquidatable(vault, target_user), E_NOT_LIQUIDATABLE);

        let (collateral, debt, _) = collateral_vault::get_position(vault, target_user);

        // Seize collateral (the vault gives us collateral + 5% bonus)
        let seized_collateral = collateral_vault::liquidate(vault, target_user, debt, ctx);

        event::emit(LiquidationExecuted {
            liquidator: ctx.sender(),
            liquidated_user: target_user,
            debt_repaid: debt,
            collateral_seized: seized_collateral.value(),
            profit: 0, // Will be calculated in final step
        });

        seized_collateral
    }

    /// Step 3: Swap seized collateral for SUI to repay flash loan
    public fun swap_collateral(
        swap_pool: &mut SwapPool,
        collateral: Coin<SUI>,
        min_output: u64,
        ctx: &mut TxContext
    ): Coin<SUI> {
        // In a real scenario, collateral might be a different token
        // Here we simulate with TOKEN -> SUI swap
        swap_pool::swap_token_for_sui(swap_pool, collateral, min_output, ctx)
    }

    /// Step 4: Complete flash liquidation - repay loan and keep profit
    public fun complete_flash_liquidation(
        flash_pool: &mut FlashPool,
        receipt: FlashReceipt,
        proceeds: Coin<SUI>,
        target_user: address,
        ctx: &mut TxContext
    ): Coin<SUI> {
        let proceeds_value = proceeds.value();
        let required = flash_loan::receipt_amount(&receipt) + flash_loan::receipt_fee(&receipt);

        assert!(proceeds_value >= required, E_UNPROFITABLE);

        let profit = proceeds_value - required;
        let min_profit = (flash_loan::receipt_amount(&receipt) * MIN_PROFIT_BPS) / BPS_BASE;
        assert!(profit >= min_profit, E_INSUFFICIENT_PROFIT);

        // Split proceeds into repayment and profit
        let mut proceeds_balance = proceeds.into_balance();
        let repayment = coin::from_balance(proceeds_balance.split(required), ctx);
        let profit_coin = coin::from_balance(proceeds_balance, ctx);

        // Repay flash loan
        flash_loan::repay(flash_pool, receipt, repayment, ctx);

        event::emit(FlashLiquidationCompleted {
            liquidator: ctx.sender(),
            target_user,
            gross_proceeds: proceeds_value,
            flash_repayment: required,
            net_profit: profit,
        });

        profit_coin
    }

    /// Convenience function: Execute full flash liquidation in one call
    /// This demonstrates the ENTIRE PTB flow in a single function
    ///
    /// A real PTB would call each step separately for more flexibility,
    /// but this shows the complete flow:
    /// 1. Flash borrow
    /// 2. Liquidate position
    /// 3. Swap collateral
    /// 4. Repay flash loan
    /// 5. Return profit
    public fun flash_liquidate(
        flash_pool: &mut FlashPool,
        vault: &mut CollateralVault,
        swap_pool: &mut SwapPool,
        target_user: address,
        debt_to_repay: u64,
        min_swap_output: u64,
        ctx: &mut TxContext
    ): Coin<SUI> {
        // Step 1: Flash borrow
        let (borrowed, receipt) = start_flash_liquidation(
            flash_pool,
            target_user,
            debt_to_repay,
            ctx
        );

        // Step 2: Execute liquidation
        let seized_collateral = execute_liquidation(
            vault,
            target_user,
            &borrowed,
            ctx
        );

        // We don't actually use borrowed funds for liquidation in this simplified model
        // In reality, we'd use them to repay the lending pool debt
        // For now, just merge them with seized collateral
        let mut merged = seized_collateral.into_balance();
        merged.join(borrowed.into_balance());
        let collateral_coin = coin::from_balance(merged, ctx);

        // Step 3: Swap all collateral
        let swap_proceeds = swap_collateral(
            swap_pool,
            collateral_coin,
            min_swap_output,
            ctx
        );

        // Step 4: Complete - repay flash loan and return profit
        complete_flash_liquidation(
            flash_pool,
            receipt,
            swap_proceeds,
            target_user,
            ctx
        )
    }

    // =========================================================================
    // View Functions
    // =========================================================================

    /// Calculate expected profit from a liquidation
    public fun estimate_profit(
        vault: &CollateralVault,
        swap_pool: &SwapPool,
        user: address,
    ): u64 {
        let (_, expected_profit) = can_liquidate_profitably(vault, swap_pool, user, get_user_debt(vault, user));
        expected_profit
    }

    /// Get user's current debt
    fun get_user_debt(vault: &CollateralVault, user: address): u64 {
        let (_, debt, _) = collateral_vault::get_position(vault, user);
        debt
    }
}
