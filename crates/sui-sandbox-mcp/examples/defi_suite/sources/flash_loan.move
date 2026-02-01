/// Flash Loan Module
///
/// Provides uncollateralized loans that must be repaid within the same PTB.
/// Uses a "hot potato" pattern - the FlashReceipt must be consumed by repayment.
///
/// # PTB Integration
/// Flash loans enable complex arbitrage strategies in a single transaction:
/// 1. Borrow from flash loan pool
/// 2. Execute arbitrage (swap, liquidate, etc.)
/// 3. Repay loan + fee
/// 4. Keep profit
#[allow(unused_field, unused_const)]
module defi_suite::flash_loan {
    use sui::coin::{Self, Coin};
    use sui::balance::{Self, Balance};
    use sui::sui::SUI;
    use sui::event;

    // =========================================================================
    // Error Codes
    // =========================================================================

    const E_INSUFFICIENT_LIQUIDITY: u64 = 100;
    const E_INVALID_REPAYMENT: u64 = 101;
    const E_POOL_PAUSED: u64 = 102;

    // =========================================================================
    // Constants
    // =========================================================================

    /// Flash loan fee in basis points (0.09% like Aave)
    const FLASH_FEE_BPS: u64 = 9;
    const BPS_BASE: u64 = 10000;

    // =========================================================================
    // Types
    // =========================================================================

    /// The flash loan pool holding available liquidity
    public struct FlashPool has key {
        id: UID,
        /// Available liquidity for flash loans
        liquidity: Balance<SUI>,
        /// Total fees collected
        fees_collected: u64,
        /// Whether pool is active
        active: bool,
    }

    /// Hot potato receipt - MUST be consumed by repaying the loan
    /// Cannot be stored, dropped, or transferred - forces same-PTB repayment
    public struct FlashReceipt {
        /// Pool ID for validation
        pool_id: ID,
        /// Amount borrowed
        amount: u64,
        /// Fee owed
        fee: u64,
    }

    /// Admin capability for pool management
    public struct FlashPoolAdmin has key, store {
        id: UID,
        pool_id: ID,
    }

    // =========================================================================
    // Events
    // =========================================================================

    public struct FlashLoanTaken has copy, drop {
        pool_id: ID,
        borrower: address,
        amount: u64,
        fee: u64,
    }

    public struct FlashLoanRepaid has copy, drop {
        pool_id: ID,
        borrower: address,
        amount: u64,
        fee: u64,
    }

    public struct LiquidityAdded has copy, drop {
        pool_id: ID,
        provider: address,
        amount: u64,
    }

    // =========================================================================
    // Pool Creation
    // =========================================================================

    /// Create a new flash loan pool with initial liquidity
    public fun create_pool(
        initial_liquidity: Coin<SUI>,
        ctx: &mut TxContext
    ): (FlashPool, FlashPoolAdmin) {
        let pool_id = object::new(ctx);
        let id_copy = pool_id.to_inner();

        let pool = FlashPool {
            id: pool_id,
            liquidity: initial_liquidity.into_balance(),
            fees_collected: 0,
            active: true,
        };

        let admin = FlashPoolAdmin {
            id: object::new(ctx),
            pool_id: id_copy,
        };

        (pool, admin)
    }

    /// Entry: Create pool and share it
    entry fun create_pool_entry(
        initial_liquidity: Coin<SUI>,
        ctx: &mut TxContext
    ) {
        let (pool, admin) = create_pool(initial_liquidity, ctx);
        transfer::share_object(pool);
        transfer::transfer(admin, ctx.sender());
    }

    // =========================================================================
    // Flash Loan Operations
    // =========================================================================

    /// Borrow funds via flash loan - returns funds AND a receipt that must be repaid
    /// This is the key PTB primitive - the receipt is a hot potato
    public fun borrow(
        pool: &mut FlashPool,
        amount: u64,
        ctx: &mut TxContext
    ): (Coin<SUI>, FlashReceipt) {
        assert!(pool.active, E_POOL_PAUSED);
        assert!(pool.liquidity.value() >= amount, E_INSUFFICIENT_LIQUIDITY);

        let fee = (amount * FLASH_FEE_BPS) / BPS_BASE;

        let borrowed = coin::from_balance(
            pool.liquidity.split(amount),
            ctx
        );

        let receipt = FlashReceipt {
            pool_id: object::id(pool),
            amount,
            fee,
        };

        event::emit(FlashLoanTaken {
            pool_id: object::id(pool),
            borrower: ctx.sender(),
            amount,
            fee,
        });

        (borrowed, receipt)
    }

    /// Repay flash loan - consumes the receipt (hot potato pattern)
    /// The repayment must include principal + fee
    public fun repay(
        pool: &mut FlashPool,
        receipt: FlashReceipt,
        repayment: Coin<SUI>,
        ctx: &TxContext
    ) {
        let FlashReceipt { pool_id, amount, fee } = receipt;

        assert!(pool_id == object::id(pool), E_INVALID_REPAYMENT);

        let required = amount + fee;
        assert!(repayment.value() >= required, E_INVALID_REPAYMENT);

        // Add repayment to pool
        pool.liquidity.join(repayment.into_balance());
        pool.fees_collected = pool.fees_collected + fee;

        event::emit(FlashLoanRepaid {
            pool_id,
            borrower: ctx.sender(),
            amount,
            fee,
        });
    }

    // =========================================================================
    // Liquidity Management
    // =========================================================================

    /// Add liquidity to the flash loan pool
    public fun add_liquidity(
        pool: &mut FlashPool,
        liquidity: Coin<SUI>,
        ctx: &TxContext
    ) {
        let amount = liquidity.value();
        pool.liquidity.join(liquidity.into_balance());

        event::emit(LiquidityAdded {
            pool_id: object::id(pool),
            provider: ctx.sender(),
            amount,
        });
    }

    entry fun add_liquidity_entry(
        pool: &mut FlashPool,
        liquidity: Coin<SUI>,
        ctx: &TxContext
    ) {
        add_liquidity(pool, liquidity, ctx);
    }

    // =========================================================================
    // View Functions
    // =========================================================================

    /// Get available liquidity
    public fun available_liquidity(pool: &FlashPool): u64 {
        pool.liquidity.value()
    }

    /// Calculate fee for a given borrow amount
    public fun calculate_fee(amount: u64): u64 {
        (amount * FLASH_FEE_BPS) / BPS_BASE
    }

    /// Get receipt details
    public fun receipt_amount(receipt: &FlashReceipt): u64 {
        receipt.amount
    }

    public fun receipt_fee(receipt: &FlashReceipt): u64 {
        receipt.fee
    }
}
