/// Fee Distributor Module
///
/// Collects fees from all protocol components and distributes to stakeholders.
/// Integrates with flash loan pool, lending pool, and swap pool.
///
/// # PTB Integration
/// Fee operations can be combined with:
/// - Protocol admin actions (collect and distribute in one tx)
/// - Staking (stake and claim fees atomically)
/// - Governance (vote and claim in same tx)
#[allow(unused_field, unused_const)]
module defi_suite::fee_distributor {
    use sui::coin::{Self, Coin};
    use sui::balance::{Self, Balance};
    use sui::sui::SUI;
    use sui::event;
    use sui::table::{Self, Table};

    // =========================================================================
    // Error Codes
    // =========================================================================

    const E_NO_STAKE: u64 = 500;
    const E_INSUFFICIENT_BALANCE: u64 = 501;
    const E_ALREADY_CLAIMED: u64 = 502;
    const E_UNAUTHORIZED: u64 = 503;

    // =========================================================================
    // Types
    // =========================================================================

    /// The fee distributor treasury
    public struct FeeDistributor has key {
        id: UID,
        /// Accumulated fees from all sources
        treasury: Balance<SUI>,
        /// Total fees ever collected
        total_collected: u64,
        /// Total fees distributed
        total_distributed: u64,
        /// Staker shares: address -> stake amount
        stakes: Table<address, u64>,
        /// Total staked amount
        total_staked: u64,
        /// Current epoch for distribution
        current_epoch: u64,
        /// Fees per epoch
        fees_per_epoch: Table<u64, u64>,
        /// Last claimed epoch per user
        last_claimed: Table<address, u64>,
    }

    /// Admin capability
    public struct DistributorAdmin has key, store {
        id: UID,
        distributor_id: ID,
    }

    /// Stake receipt
    public struct StakeReceipt has key, store {
        id: UID,
        distributor_id: ID,
        staker: address,
        amount: u64,
        staked_at_epoch: u64,
    }

    // =========================================================================
    // Events
    // =========================================================================

    public struct DistributorCreated has copy, drop {
        distributor_id: ID,
    }

    public struct FeesCollected has copy, drop {
        distributor_id: ID,
        source: vector<u8>,
        amount: u64,
        epoch: u64,
    }

    public struct Staked has copy, drop {
        distributor_id: ID,
        staker: address,
        amount: u64,
        total_stake: u64,
    }

    public struct Unstaked has copy, drop {
        distributor_id: ID,
        staker: address,
        amount: u64,
        remaining_stake: u64,
    }

    public struct FeesClaimed has copy, drop {
        distributor_id: ID,
        staker: address,
        amount: u64,
        epochs_claimed: u64,
    }

    public struct EpochAdvanced has copy, drop {
        distributor_id: ID,
        new_epoch: u64,
        fees_in_epoch: u64,
    }

    // =========================================================================
    // Distributor Creation
    // =========================================================================

    /// Create a new fee distributor
    public fun create_distributor(ctx: &mut TxContext): (FeeDistributor, DistributorAdmin) {
        let dist_id = object::new(ctx);
        let id_copy = dist_id.to_inner();

        let distributor = FeeDistributor {
            id: dist_id,
            treasury: balance::zero(),
            total_collected: 0,
            total_distributed: 0,
            stakes: table::new(ctx),
            total_staked: 0,
            current_epoch: 0,
            fees_per_epoch: table::new(ctx),
            last_claimed: table::new(ctx),
        };

        let admin = DistributorAdmin {
            id: object::new(ctx),
            distributor_id: id_copy,
        };

        event::emit(DistributorCreated {
            distributor_id: id_copy,
        });

        (distributor, admin)
    }

    entry fun create_distributor_entry(ctx: &mut TxContext) {
        let (dist, admin) = create_distributor(ctx);
        transfer::share_object(dist);
        transfer::transfer(admin, ctx.sender());
    }

    // =========================================================================
    // Fee Collection
    // =========================================================================

    /// Collect fees from a protocol component
    public fun collect_fees(
        distributor: &mut FeeDistributor,
        fees: Coin<SUI>,
        source: vector<u8>,
    ) {
        let amount = fees.value();
        distributor.treasury.join(fees.into_balance());
        distributor.total_collected = distributor.total_collected + amount;

        // Add to current epoch fees
        let epoch = distributor.current_epoch;
        if (!distributor.fees_per_epoch.contains(epoch)) {
            distributor.fees_per_epoch.add(epoch, amount);
        } else {
            let current = distributor.fees_per_epoch.borrow_mut(epoch);
            *current = *current + amount;
        };

        event::emit(FeesCollected {
            distributor_id: object::id(distributor),
            source,
            amount,
            epoch,
        });
    }

    entry fun collect_fees_entry(
        distributor: &mut FeeDistributor,
        fees: Coin<SUI>,
        source: vector<u8>,
    ) {
        collect_fees(distributor, fees, source);
    }

    // =========================================================================
    // Staking Operations
    // =========================================================================

    /// Stake to earn fee share
    public fun stake(
        distributor: &mut FeeDistributor,
        stake_coin: Coin<SUI>,
        ctx: &mut TxContext
    ): StakeReceipt {
        let amount = stake_coin.value();
        let staker = ctx.sender();

        // Add to stakes
        if (!distributor.stakes.contains(staker)) {
            distributor.stakes.add(staker, amount);
            distributor.last_claimed.add(staker, distributor.current_epoch);
        } else {
            let current = distributor.stakes.borrow_mut(staker);
            *current = *current + amount;
        };

        distributor.total_staked = distributor.total_staked + amount;
        distributor.treasury.join(stake_coin.into_balance());

        event::emit(Staked {
            distributor_id: object::id(distributor),
            staker,
            amount,
            total_stake: *distributor.stakes.borrow(staker),
        });

        StakeReceipt {
            id: object::new(ctx),
            distributor_id: object::id(distributor),
            staker,
            amount,
            staked_at_epoch: distributor.current_epoch,
        }
    }

    /// Unstake and withdraw
    public fun unstake(
        distributor: &mut FeeDistributor,
        receipt: StakeReceipt,
        ctx: &mut TxContext
    ): Coin<SUI> {
        let StakeReceipt { id, distributor_id: _, staker, amount, staked_at_epoch: _ } = receipt;
        id.delete();

        assert!(distributor.stakes.contains(staker), E_NO_STAKE);

        let remaining_stake = {
            let current_stake = distributor.stakes.borrow_mut(staker);
            assert!(*current_stake >= amount, E_INSUFFICIENT_BALANCE);
            *current_stake = *current_stake - amount;
            *current_stake
        };

        distributor.total_staked = distributor.total_staked - amount;

        event::emit(Unstaked {
            distributor_id: object::id(distributor),
            staker,
            amount,
            remaining_stake,
        });

        coin::from_balance(distributor.treasury.split(amount), ctx)
    }

    // =========================================================================
    // Fee Distribution
    // =========================================================================

    /// Claim accumulated fees based on stake share
    public fun claim_fees(
        distributor: &mut FeeDistributor,
        ctx: &mut TxContext
    ): Coin<SUI> {
        let staker = ctx.sender();
        assert!(distributor.stakes.contains(staker), E_NO_STAKE);

        let stake = *distributor.stakes.borrow(staker);
        let last_epoch = *distributor.last_claimed.borrow(staker);
        let current_epoch = distributor.current_epoch;

        // Calculate claimable fees across epochs
        let mut claimable: u64 = 0;
        let mut epoch = last_epoch + 1;

        while (epoch <= current_epoch) {
            if (distributor.fees_per_epoch.contains(epoch)) {
                let epoch_fees = *distributor.fees_per_epoch.borrow(epoch);
                // User's share = (their stake / total staked) * epoch fees
                if (distributor.total_staked > 0) {
                    claimable = claimable + (stake * epoch_fees) / distributor.total_staked;
                };
            };
            epoch = epoch + 1;
        };

        // Update last claimed
        let last = distributor.last_claimed.borrow_mut(staker);
        *last = current_epoch;

        distributor.total_distributed = distributor.total_distributed + claimable;

        event::emit(FeesClaimed {
            distributor_id: object::id(distributor),
            staker,
            amount: claimable,
            epochs_claimed: current_epoch - last_epoch,
        });

        if (claimable > 0) {
            coin::from_balance(distributor.treasury.split(claimable), ctx)
        } else {
            coin::zero(ctx)
        }
    }

    entry fun claim_fees_entry(
        distributor: &mut FeeDistributor,
        ctx: &mut TxContext
    ) {
        let fees = claim_fees(distributor, ctx);
        if (fees.value() > 0) {
            transfer::public_transfer(fees, ctx.sender());
        } else {
            fees.destroy_zero();
        };
    }

    /// Advance to next epoch (admin only for simplicity)
    public fun advance_epoch(
        distributor: &mut FeeDistributor,
        _admin: &DistributorAdmin,
    ) {
        let fees_in_epoch = if (distributor.fees_per_epoch.contains(distributor.current_epoch)) {
            *distributor.fees_per_epoch.borrow(distributor.current_epoch)
        } else {
            0
        };

        distributor.current_epoch = distributor.current_epoch + 1;

        event::emit(EpochAdvanced {
            distributor_id: object::id(distributor),
            new_epoch: distributor.current_epoch,
            fees_in_epoch,
        });
    }

    entry fun advance_epoch_entry(
        distributor: &mut FeeDistributor,
        admin: &DistributorAdmin,
    ) {
        advance_epoch(distributor, admin);
    }

    // =========================================================================
    // View Functions
    // =========================================================================

    /// Get distributor stats
    public fun get_stats(distributor: &FeeDistributor): (u64, u64, u64, u64) {
        (
            distributor.treasury.value(),
            distributor.total_collected,
            distributor.total_distributed,
            distributor.total_staked
        )
    }

    /// Get user's stake
    public fun get_stake(distributor: &FeeDistributor, user: address): u64 {
        if (distributor.stakes.contains(user)) {
            *distributor.stakes.borrow(user)
        } else {
            0
        }
    }

    /// Get current epoch
    public fun current_epoch(distributor: &FeeDistributor): u64 {
        distributor.current_epoch
    }
}
