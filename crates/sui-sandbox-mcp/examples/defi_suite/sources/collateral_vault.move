/// Collateral Vault Module
///
/// Manages user collateral deposits with health factor tracking.
/// Integrates with lending pool for borrowing and liquidation engine for bad debt.
///
/// # PTB Integration
/// Collateral operations can be chained with:
/// - Flash loans (deposit flash-borrowed funds as collateral)
/// - Lending (borrow against deposited collateral)
/// - Liquidations (seize collateral from underwater positions)
#[allow(unused_field, unused_const)]
module defi_suite::collateral_vault {
    use sui::coin::{Self, Coin};
    use sui::balance::{Self, Balance};
    use sui::sui::SUI;
    use sui::event;
    use sui::table::{Self, Table};

    // =========================================================================
    // Error Codes
    // =========================================================================

    const E_INSUFFICIENT_COLLATERAL: u64 = 200;
    const E_HEALTH_FACTOR_TOO_LOW: u64 = 201;
    const E_NO_POSITION: u64 = 202;
    const E_UNAUTHORIZED: u64 = 203;
    const E_VAULT_PAUSED: u64 = 204;

    // =========================================================================
    // Constants
    // =========================================================================

    /// Collateral factor (80% = can borrow 80% of collateral value)
    const COLLATERAL_FACTOR_BPS: u64 = 8000;
    /// Liquidation threshold (85% = liquidate when debt >= 85% of collateral)
    const LIQUIDATION_THRESHOLD_BPS: u64 = 8500;
    /// Minimum health factor (1.0 = 10000 in BPS)
    const MIN_HEALTH_FACTOR: u64 = 10000;
    const BPS_BASE: u64 = 10000;

    // =========================================================================
    // Types
    // =========================================================================

    /// The main vault holding all collateral
    public struct CollateralVault has key {
        id: UID,
        /// Total collateral held
        total_collateral: Balance<SUI>,
        /// User positions: address -> CollateralPosition
        positions: Table<address, CollateralPosition>,
        /// Whether vault is active
        active: bool,
    }

    /// Individual user's collateral position
    public struct CollateralPosition has store {
        /// Deposited collateral amount
        collateral: u64,
        /// Current debt (set by lending pool)
        debt: u64,
    }

    /// Admin capability
    public struct VaultAdmin has key, store {
        id: UID,
        vault_id: ID,
    }

    /// Proof of collateral for lending pool integration
    /// Returned when collateral is locked for borrowing
    public struct CollateralProof has store, drop {
        vault_id: ID,
        user: address,
        collateral_value: u64,
        max_borrow: u64,
    }

    // =========================================================================
    // Events
    // =========================================================================

    public struct CollateralDeposited has copy, drop {
        vault_id: ID,
        user: address,
        amount: u64,
        total_collateral: u64,
    }

    public struct CollateralWithdrawn has copy, drop {
        vault_id: ID,
        user: address,
        amount: u64,
        remaining_collateral: u64,
    }

    public struct DebtUpdated has copy, drop {
        vault_id: ID,
        user: address,
        new_debt: u64,
        health_factor: u64,
    }

    public struct PositionLiquidated has copy, drop {
        vault_id: ID,
        user: address,
        collateral_seized: u64,
        debt_repaid: u64,
        liquidator: address,
    }

    // =========================================================================
    // Vault Creation
    // =========================================================================

    /// Create a new collateral vault
    public fun create_vault(ctx: &mut TxContext): (CollateralVault, VaultAdmin) {
        let vault_id = object::new(ctx);
        let id_copy = vault_id.to_inner();

        let vault = CollateralVault {
            id: vault_id,
            total_collateral: balance::zero(),
            positions: table::new(ctx),
            active: true,
        };

        let admin = VaultAdmin {
            id: object::new(ctx),
            vault_id: id_copy,
        };

        (vault, admin)
    }

    entry fun create_vault_entry(ctx: &mut TxContext) {
        let (vault, admin) = create_vault(ctx);
        transfer::share_object(vault);
        transfer::transfer(admin, ctx.sender());
    }

    // =========================================================================
    // Collateral Operations
    // =========================================================================

    /// Deposit collateral into the vault
    public fun deposit(
        vault: &mut CollateralVault,
        collateral: Coin<SUI>,
        ctx: &mut TxContext
    ) {
        assert!(vault.active, E_VAULT_PAUSED);

        let amount = collateral.value();
        let user = ctx.sender();

        vault.total_collateral.join(collateral.into_balance());

        if (!vault.positions.contains(user)) {
            vault.positions.add(user, CollateralPosition {
                collateral: amount,
                debt: 0,
            });
        } else {
            let position = vault.positions.borrow_mut(user);
            position.collateral = position.collateral + amount;
        };

        let total = vault.positions.borrow(user).collateral;

        event::emit(CollateralDeposited {
            vault_id: object::id(vault),
            user,
            amount,
            total_collateral: total,
        });
    }

    entry fun deposit_entry(
        vault: &mut CollateralVault,
        collateral: Coin<SUI>,
        ctx: &mut TxContext
    ) {
        deposit(vault, collateral, ctx);
    }

    /// Withdraw collateral (must maintain health factor)
    public fun withdraw(
        vault: &mut CollateralVault,
        amount: u64,
        ctx: &mut TxContext
    ): Coin<SUI> {
        assert!(vault.active, E_VAULT_PAUSED);

        let user = ctx.sender();
        assert!(vault.positions.contains(user), E_NO_POSITION);

        let position = vault.positions.borrow_mut(user);
        assert!(position.collateral >= amount, E_INSUFFICIENT_COLLATERAL);

        // Check health factor after withdrawal
        let new_collateral = position.collateral - amount;
        if (position.debt > 0) {
            let new_hf = calculate_health_factor(new_collateral, position.debt);
            assert!(new_hf >= MIN_HEALTH_FACTOR, E_HEALTH_FACTOR_TOO_LOW);
        };

        position.collateral = new_collateral;

        event::emit(CollateralWithdrawn {
            vault_id: object::id(vault),
            user,
            amount,
            remaining_collateral: new_collateral,
        });

        coin::from_balance(vault.total_collateral.split(amount), ctx)
    }

    entry fun withdraw_entry(
        vault: &mut CollateralVault,
        amount: u64,
        ctx: &mut TxContext
    ) {
        let coin = withdraw(vault, amount, ctx);
        transfer::public_transfer(coin, ctx.sender());
    }

    // =========================================================================
    // Lending Integration
    // =========================================================================

    /// Get proof of collateral for lending pool
    /// Returns how much the user can borrow
    public fun get_collateral_proof(
        vault: &CollateralVault,
        ctx: &TxContext
    ): CollateralProof {
        let user = ctx.sender();

        let (collateral, debt) = if (vault.positions.contains(user)) {
            let pos = vault.positions.borrow(user);
            (pos.collateral, pos.debt)
        } else {
            (0, 0)
        };

        let max_borrow = (collateral * COLLATERAL_FACTOR_BPS / BPS_BASE) - debt;

        CollateralProof {
            vault_id: object::id(vault),
            user,
            collateral_value: collateral,
            max_borrow,
        }
    }

    /// Update debt for a user (called by lending pool)
    public fun update_debt(
        vault: &mut CollateralVault,
        user: address,
        new_debt: u64,
    ) {
        assert!(vault.positions.contains(user), E_NO_POSITION);

        let position = vault.positions.borrow_mut(user);
        position.debt = new_debt;

        let hf = calculate_health_factor(position.collateral, new_debt);

        event::emit(DebtUpdated {
            vault_id: object::id(vault),
            user,
            new_debt,
            health_factor: hf,
        });
    }

    // =========================================================================
    // Liquidation Integration
    // =========================================================================

    /// Check if a position is liquidatable
    public fun is_liquidatable(vault: &CollateralVault, user: address): bool {
        if (!vault.positions.contains(user)) {
            return false
        };

        let position = vault.positions.borrow(user);
        if (position.debt == 0) {
            return false
        };

        let hf = calculate_health_factor(position.collateral, position.debt);
        hf < MIN_HEALTH_FACTOR
    }

    /// Execute liquidation - seize collateral and reduce debt
    /// Returns the seized collateral
    public fun liquidate(
        vault: &mut CollateralVault,
        user: address,
        debt_to_repay: u64,
        ctx: &mut TxContext
    ): Coin<SUI> {
        assert!(is_liquidatable(vault, user), E_HEALTH_FACTOR_TOO_LOW);

        let position = vault.positions.borrow_mut(user);

        // Liquidation bonus: 5% extra collateral
        let collateral_to_seize = debt_to_repay + (debt_to_repay / 20);
        let actual_seize = if (collateral_to_seize > position.collateral) {
            position.collateral
        } else {
            collateral_to_seize
        };

        position.collateral = position.collateral - actual_seize;
        position.debt = if (debt_to_repay > position.debt) {
            0
        } else {
            position.debt - debt_to_repay
        };

        event::emit(PositionLiquidated {
            vault_id: object::id(vault),
            user,
            collateral_seized: actual_seize,
            debt_repaid: debt_to_repay,
            liquidator: ctx.sender(),
        });

        coin::from_balance(vault.total_collateral.split(actual_seize), ctx)
    }

    // =========================================================================
    // View Functions
    // =========================================================================

    /// Calculate health factor: (collateral * liquidation_threshold) / debt
    public fun calculate_health_factor(collateral: u64, debt: u64): u64 {
        if (debt == 0) {
            return 0xFFFFFFFF // Max u64 represents infinite health
        };
        (collateral * LIQUIDATION_THRESHOLD_BPS) / debt
    }

    /// Get user's position
    public fun get_position(vault: &CollateralVault, user: address): (u64, u64, u64) {
        if (!vault.positions.contains(user)) {
            return (0, 0, 0xFFFFFFFF)
        };

        let position = vault.positions.borrow(user);
        let hf = calculate_health_factor(position.collateral, position.debt);
        (position.collateral, position.debt, hf)
    }

    /// Get collateral proof details
    public fun proof_max_borrow(proof: &CollateralProof): u64 {
        proof.max_borrow
    }

    public fun proof_collateral_value(proof: &CollateralProof): u64 {
        proof.collateral_value
    }
}
