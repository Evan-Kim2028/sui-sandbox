/// Lending Pool Module
///
/// Enables borrowing against collateral deposited in CollateralVault.
/// Tracks interest accrual and integrates with liquidation engine.
///
/// # PTB Integration
/// Lending operations can be combined with:
/// - Collateral vault (deposit collateral, then borrow)
/// - Flash loans (borrow flash loan to repay lending debt)
/// - Swaps (borrow asset, swap for another)
#[allow(unused_field, unused_const)]
module defi_suite::lending_pool {
    use sui::coin::{Self, Coin};
    use sui::balance::{Self, Balance};
    use sui::sui::SUI;
    use sui::event;
    use sui::clock::Clock;
    use defi_suite::collateral_vault::{Self, CollateralVault, CollateralProof};

    // =========================================================================
    // Error Codes
    // =========================================================================

    const E_INSUFFICIENT_LIQUIDITY: u64 = 300;
    const E_EXCEEDS_BORROW_LIMIT: u64 = 301;
    const E_NO_DEBT: u64 = 302;
    const E_POOL_PAUSED: u64 = 303;
    const E_INVALID_PROOF: u64 = 304;

    // =========================================================================
    // Constants
    // =========================================================================

    /// Annual interest rate in basis points (5% APR)
    const INTEREST_RATE_BPS: u64 = 500;
    /// Seconds per year for interest calculation
    const SECONDS_PER_YEAR: u64 = 31536000;
    const BPS_BASE: u64 = 10000;

    // =========================================================================
    // Types
    // =========================================================================

    /// The lending pool
    public struct LendingPool has key {
        id: UID,
        /// Available liquidity for borrowing
        liquidity: Balance<SUI>,
        /// Total borrowed amount
        total_borrowed: u64,
        /// Total interest accrued
        total_interest: u64,
        /// Last update timestamp
        last_update_ms: u64,
        /// Whether pool is active
        active: bool,
    }

    /// Debt receipt - represents user's debt position
    public struct DebtReceipt has key, store {
        id: UID,
        pool_id: ID,
        borrower: address,
        principal: u64,
        interest_accrued: u64,
        borrow_timestamp_ms: u64,
    }

    /// Admin capability
    public struct LendingPoolAdmin has key, store {
        id: UID,
        pool_id: ID,
    }

    // =========================================================================
    // Events
    // =========================================================================

    public struct PoolCreated has copy, drop {
        pool_id: ID,
        initial_liquidity: u64,
    }

    public struct Borrowed has copy, drop {
        pool_id: ID,
        borrower: address,
        amount: u64,
        receipt_id: ID,
    }

    public struct Repaid has copy, drop {
        pool_id: ID,
        borrower: address,
        principal_repaid: u64,
        interest_paid: u64,
    }

    public struct LiquidityProvided has copy, drop {
        pool_id: ID,
        provider: address,
        amount: u64,
    }

    // =========================================================================
    // Pool Creation
    // =========================================================================

    /// Create a new lending pool
    public fun create_pool(
        initial_liquidity: Coin<SUI>,
        clock: &Clock,
        ctx: &mut TxContext
    ): (LendingPool, LendingPoolAdmin) {
        let pool_id = object::new(ctx);
        let id_copy = pool_id.to_inner();
        let amount = initial_liquidity.value();

        let pool = LendingPool {
            id: pool_id,
            liquidity: initial_liquidity.into_balance(),
            total_borrowed: 0,
            total_interest: 0,
            last_update_ms: clock.timestamp_ms(),
            active: true,
        };

        let admin = LendingPoolAdmin {
            id: object::new(ctx),
            pool_id: id_copy,
        };

        event::emit(PoolCreated {
            pool_id: id_copy,
            initial_liquidity: amount,
        });

        (pool, admin)
    }

    entry fun create_pool_entry(
        initial_liquidity: Coin<SUI>,
        clock: &Clock,
        ctx: &mut TxContext
    ) {
        let (pool, admin) = create_pool(initial_liquidity, clock, ctx);
        transfer::share_object(pool);
        transfer::transfer(admin, ctx.sender());
    }

    // =========================================================================
    // Borrowing Operations
    // =========================================================================

    /// Borrow against collateral
    /// Requires a CollateralProof from the collateral vault
    public fun borrow(
        pool: &mut LendingPool,
        vault: &mut CollateralVault,
        proof: CollateralProof,
        amount: u64,
        clock: &Clock,
        ctx: &mut TxContext
    ): (Coin<SUI>, DebtReceipt) {
        assert!(pool.active, E_POOL_PAUSED);
        assert!(pool.liquidity.value() >= amount, E_INSUFFICIENT_LIQUIDITY);
        assert!(amount <= collateral_vault::proof_max_borrow(&proof), E_EXCEEDS_BORROW_LIMIT);

        let borrower = ctx.sender();

        // Update collateral vault with new debt
        let (current_collateral, current_debt, _) = collateral_vault::get_position(vault, borrower);
        collateral_vault::update_debt(vault, borrower, current_debt + amount);

        // Take liquidity
        let borrowed = coin::from_balance(pool.liquidity.split(amount), ctx);
        pool.total_borrowed = pool.total_borrowed + amount;

        // Create debt receipt
        let receipt_id = object::new(ctx);
        let receipt_id_copy = receipt_id.to_inner();

        let receipt = DebtReceipt {
            id: receipt_id,
            pool_id: object::id(pool),
            borrower,
            principal: amount,
            interest_accrued: 0,
            borrow_timestamp_ms: clock.timestamp_ms(),
        };

        event::emit(Borrowed {
            pool_id: object::id(pool),
            borrower,
            amount,
            receipt_id: receipt_id_copy,
        });

        (borrowed, receipt)
    }

    /// Repay debt (partial or full)
    public fun repay(
        pool: &mut LendingPool,
        vault: &mut CollateralVault,
        receipt: &mut DebtReceipt,
        payment: Coin<SUI>,
        clock: &Clock,
        ctx: &mut TxContext
    ): Option<Coin<SUI>> {
        // Calculate current interest
        let elapsed_ms = clock.timestamp_ms() - receipt.borrow_timestamp_ms;
        let elapsed_seconds = elapsed_ms / 1000;
        let interest = (receipt.principal * INTEREST_RATE_BPS * elapsed_seconds) / (BPS_BASE * SECONDS_PER_YEAR);
        receipt.interest_accrued = interest;

        let total_owed = receipt.principal + interest;
        let payment_amount = payment.value();

        let (principal_repaid, interest_paid, refund) = if (payment_amount >= total_owed) {
            // Full repayment
            let principal = receipt.principal;
            let int = interest;
            receipt.principal = 0;
            receipt.interest_accrued = 0;

            // Refund excess
            let refund_amount = payment_amount - total_owed;
            pool.liquidity.join(payment.into_balance());

            if (refund_amount > 0) {
                (principal, int, option::some(coin::from_balance(pool.liquidity.split(refund_amount), ctx)))
            } else {
                (principal, int, option::none())
            }
        } else {
            // Partial repayment - pay interest first, then principal
            let int_paid = if (payment_amount >= interest) { interest } else { payment_amount };
            let principal_paid = payment_amount - int_paid;

            receipt.principal = receipt.principal - principal_paid;
            receipt.interest_accrued = interest - int_paid;

            pool.liquidity.join(payment.into_balance());
            (principal_paid, int_paid, option::none())
        };

        // Update pool totals
        pool.total_borrowed = pool.total_borrowed - principal_repaid;
        pool.total_interest = pool.total_interest + interest_paid;

        // Update vault debt
        collateral_vault::update_debt(vault, receipt.borrower, receipt.principal);

        event::emit(Repaid {
            pool_id: object::id(pool),
            borrower: receipt.borrower,
            principal_repaid,
            interest_paid,
        });

        refund
    }

    /// Repay full debt and destroy receipt
    public fun repay_full(
        pool: &mut LendingPool,
        vault: &mut CollateralVault,
        receipt: DebtReceipt,
        payment: Coin<SUI>,
        clock: &Clock,
        ctx: &mut TxContext
    ): Option<Coin<SUI>> {
        let DebtReceipt { id, pool_id: _, borrower, principal, interest_accrued: _, borrow_timestamp_ms } = receipt;

        // Calculate interest
        let elapsed_ms = clock.timestamp_ms() - borrow_timestamp_ms;
        let elapsed_seconds = elapsed_ms / 1000;
        let interest = (principal * INTEREST_RATE_BPS * elapsed_seconds) / (BPS_BASE * SECONDS_PER_YEAR);

        let total_owed = principal + interest;
        let payment_amount = payment.value();

        assert!(payment_amount >= total_owed, E_NO_DEBT);

        pool.liquidity.join(payment.into_balance());
        pool.total_borrowed = pool.total_borrowed - principal;
        pool.total_interest = pool.total_interest + interest;

        // Clear debt in vault
        collateral_vault::update_debt(vault, borrower, 0);

        id.delete();

        event::emit(Repaid {
            pool_id: object::id(pool),
            borrower,
            principal_repaid: principal,
            interest_paid: interest,
        });

        // Refund excess
        let refund_amount = payment_amount - total_owed;
        if (refund_amount > 0) {
            option::some(coin::from_balance(pool.liquidity.split(refund_amount), ctx))
        } else {
            option::none()
        }
    }

    // =========================================================================
    // Liquidity Management
    // =========================================================================

    /// Provide liquidity to the pool
    public fun provide_liquidity(
        pool: &mut LendingPool,
        liquidity: Coin<SUI>,
        ctx: &TxContext
    ) {
        let amount = liquidity.value();
        pool.liquidity.join(liquidity.into_balance());

        event::emit(LiquidityProvided {
            pool_id: object::id(pool),
            provider: ctx.sender(),
            amount,
        });
    }

    entry fun provide_liquidity_entry(
        pool: &mut LendingPool,
        liquidity: Coin<SUI>,
        ctx: &TxContext
    ) {
        provide_liquidity(pool, liquidity, ctx);
    }

    // =========================================================================
    // View Functions
    // =========================================================================

    /// Get pool stats
    public fun get_pool_stats(pool: &LendingPool): (u64, u64, u64) {
        (pool.liquidity.value(), pool.total_borrowed, pool.total_interest)
    }

    /// Get debt receipt details
    public fun get_debt_info(receipt: &DebtReceipt): (u64, u64, address) {
        (receipt.principal, receipt.interest_accrued, receipt.borrower)
    }

    /// Calculate current debt with interest
    public fun calculate_current_debt(receipt: &DebtReceipt, clock: &Clock): u64 {
        let elapsed_ms = clock.timestamp_ms() - receipt.borrow_timestamp_ms;
        let elapsed_seconds = elapsed_ms / 1000;
        let interest = (receipt.principal * INTEREST_RATE_BPS * elapsed_seconds) / (BPS_BASE * SECONDS_PER_YEAR);
        receipt.principal + interest
    }
}
