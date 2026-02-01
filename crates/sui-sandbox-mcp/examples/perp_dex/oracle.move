/// Production-Ready Oracle Module for Sui
///
/// This module provides a secure, decentralized price feed system with:
/// - Admin-controlled price updates with capability pattern
/// - Staleness checks to prevent using outdated prices
/// - Confidence intervals for price quality assessment
/// - Emergency pause functionality
/// - Event emission for off-chain tracking
///
/// # Security Features
/// - Only authorized updaters can modify prices (UpdaterCap required)
/// - Prices have timestamps to detect staleness
/// - Pause mechanism for emergency situations
/// - Immutable price history through events
///
/// # Architecture
/// - PriceFeed: Shared object containing current price data
/// - AdminCap: Capability for administrative operations
/// - UpdaterCap: Capability for price updates (can be delegated)
#[allow(duplicate_alias, unused_field)]
module perp_dex::oracle {
    use sui::event;
    use sui::clock::{Self, Clock};

    // =========================================================================
    // Error Codes
    // =========================================================================

    /// Price feed is currently paused
    const E_FEED_PAUSED: u64 = 1;
    /// Price data is stale (older than max_staleness_ms)
    const E_PRICE_STALE: u64 = 2;
    /// Invalid price value (must be > 0)
    const E_INVALID_PRICE: u64 = 3;
    /// Caller is not authorized for this operation
    const E_NOT_AUTHORIZED: u64 = 5;
    /// Invalid staleness threshold
    const E_INVALID_STALENESS: u64 = 6;

    // =========================================================================
    // Constants
    // =========================================================================

    /// Default maximum staleness: 5 minutes in milliseconds
    const DEFAULT_MAX_STALENESS_MS: u64 = 300_000;
    /// Minimum allowed staleness threshold: 10 seconds
    const MIN_STALENESS_MS: u64 = 10_000;
    /// Maximum allowed staleness threshold: 1 hour
    const MAX_STALENESS_MS: u64 = 3_600_000;
    /// Standard decimals for prices (8 decimals = 100,000,000 units per 1.0)
    const PRICE_DECIMALS: u8 = 8;

    // =========================================================================
    // Structs
    // =========================================================================

    /// Administrative capability for oracle management
    /// Holder can: pause/unpause, change staleness, create updater caps
    public struct AdminCap has key, store {
        id: UID,
        /// The feed this admin cap controls
        feed_id: ID,
    }

    /// Updater capability for price updates
    /// Can be delegated to automated systems
    public struct UpdaterCap has key, store {
        id: UID,
        /// The feed this updater can update
        feed_id: ID,
    }

    /// Price feed containing current price data
    /// Shared object - anyone can read, only UpdaterCap holders can write
    public struct PriceFeed has key, store {
        id: UID,
        /// Trading pair identifier (e.g., "BTC/USD")
        pair: vector<u8>,
        /// Current price with PRICE_DECIMALS precision
        price: u64,
        /// Confidence interval (same decimals as price)
        /// Represents uncertainty: actual price is likely within price +/- confidence
        confidence: u64,
        /// Timestamp of last update (milliseconds since epoch)
        last_update_ms: u64,
        /// Number of decimal places in price
        decimals: u8,
        /// Maximum allowed staleness before price is rejected
        max_staleness_ms: u64,
        /// Whether the feed is paused (no reads allowed)
        paused: bool,
        /// Total number of updates (for tracking)
        update_count: u64,
        /// Oracle admin address
        admin: address,
    }

    // =========================================================================
    // Events
    // =========================================================================

    /// Emitted when a new price feed is created
    public struct FeedCreated has copy, drop {
        feed_id: ID,
        pair: vector<u8>,
        initial_price: u64,
        decimals: u8,
        creator: address,
    }

    /// Emitted on every price update
    public struct PriceUpdated has copy, drop {
        feed_id: ID,
        old_price: u64,
        new_price: u64,
        confidence: u64,
        timestamp_ms: u64,
        update_count: u64,
    }

    /// Emitted when feed is paused or unpaused
    public struct FeedPauseChanged has copy, drop {
        feed_id: ID,
        paused: bool,
    }

    /// Emitted when staleness threshold changes
    public struct StalenessChanged has copy, drop {
        feed_id: ID,
        old_staleness_ms: u64,
        new_staleness_ms: u64,
    }

    /// Emitted when a new updater cap is created
    public struct UpdaterCapCreated has copy, drop {
        feed_id: ID,
        updater_cap_id: ID,
        created_by: address,
    }

    /// Emitted when an updater cap is revoked
    public struct UpdaterCapRevoked has copy, drop {
        feed_id: ID,
        updater_cap_id: ID,
    }

    /// Emitted when price is read (for analytics)
    public struct PriceRead has copy, drop {
        feed_id: ID,
        price: u64,
        confidence: u64,
        reader: address,
    }

    // =========================================================================
    // Constructor
    // =========================================================================

    /// Create a new price feed with initial price
    /// Returns both the AdminCap and UpdaterCap
    ///
    /// # Arguments
    /// * `pair` - Trading pair identifier (e.g., b"BTC/USD")
    /// * `initial_price` - Initial price with 8 decimal places
    ///
    /// # Returns
    /// * PriceFeed - The feed (caller should share or transfer)
    /// * AdminCap - Administrative capability (keep secure!)
    /// * UpdaterCap - Updater capability (can be delegated)
    public fun create_feed(
        pair: vector<u8>,
        initial_price: u64,
        ctx: &mut TxContext
    ): (PriceFeed, AdminCap, UpdaterCap) {
        assert!(initial_price > 0, E_INVALID_PRICE);

        let feed_uid = object::new(ctx);
        let feed_id = feed_uid.to_inner();

        let feed = PriceFeed {
            id: feed_uid,
            pair,
            price: initial_price,
            confidence: 0,
            last_update_ms: 0, // Will be set on first update with clock
            decimals: PRICE_DECIMALS,
            max_staleness_ms: DEFAULT_MAX_STALENESS_MS,
            paused: false,
            update_count: 1,
            admin: ctx.sender(),
        };

        event::emit(FeedCreated {
            feed_id,
            pair: feed.pair,
            initial_price,
            decimals: PRICE_DECIMALS,
            creator: ctx.sender(),
        });

        let admin_cap = AdminCap {
            id: object::new(ctx),
            feed_id,
        };

        let updater_cap = UpdaterCap {
            id: object::new(ctx),
            feed_id,
        };

        (feed, admin_cap, updater_cap)
    }

    /// Convenience entry function: create feed and transfer caps to sender
    entry fun create_feed_entry(
        pair: vector<u8>,
        initial_price: u64,
        ctx: &mut TxContext
    ) {
        let (feed, admin_cap, updater_cap) = create_feed(pair, initial_price, ctx);
        transfer::share_object(feed);
        transfer::transfer(admin_cap, ctx.sender());
        transfer::transfer(updater_cap, ctx.sender());
    }

    // =========================================================================
    // Price Update Functions
    // =========================================================================

    /// Update price with new value and confidence interval
    ///
    /// # Arguments
    /// * `feed` - The price feed to update
    /// * `cap` - Updater capability proving authorization
    /// * `new_price` - New price value (must be > 0)
    /// * `confidence` - Confidence interval
    /// * `clock` - Sui clock for timestamp
    public fun update_price(
        feed: &mut PriceFeed,
        cap: &UpdaterCap,
        new_price: u64,
        confidence: u64,
        clock: &Clock,
    ) {
        // Verify authorization
        assert!(cap.feed_id == object::id(feed), E_NOT_AUTHORIZED);
        assert!(new_price > 0, E_INVALID_PRICE);

        let old_price = feed.price;
        let timestamp = clock::timestamp_ms(clock);

        feed.price = new_price;
        feed.confidence = confidence;
        feed.last_update_ms = timestamp;
        feed.update_count = feed.update_count + 1;

        event::emit(PriceUpdated {
            feed_id: object::id(feed),
            old_price,
            new_price,
            confidence,
            timestamp_ms: timestamp,
            update_count: feed.update_count,
        });
    }

    /// Entry function for price updates
    entry fun update_price_entry(
        feed: &mut PriceFeed,
        cap: &UpdaterCap,
        new_price: u64,
        confidence: u64,
        clock: &Clock,
    ) {
        update_price(feed, cap, new_price, confidence, clock);
    }

    // =========================================================================
    // Price Read Functions (Safe)
    // =========================================================================

    /// Get price with staleness and pause checks
    /// This is the recommended way to read prices
    ///
    /// # Errors
    /// * E_FEED_PAUSED - Feed is currently paused
    /// * E_PRICE_STALE - Price is older than max_staleness_ms
    public fun get_price(feed: &PriceFeed, clock: &Clock): u64 {
        assert!(!feed.paused, E_FEED_PAUSED);

        let current_time = clock::timestamp_ms(clock);
        // Allow initial price (last_update_ms == 0) to be used
        if (feed.last_update_ms > 0) {
            let age = current_time - feed.last_update_ms;
            assert!(age <= feed.max_staleness_ms, E_PRICE_STALE);
        };

        feed.price
    }

    /// Get price with confidence interval
    /// Returns (price, confidence, timestamp_ms)
    public fun get_price_with_confidence(
        feed: &PriceFeed,
        clock: &Clock
    ): (u64, u64, u64) {
        assert!(!feed.paused, E_FEED_PAUSED);

        let current_time = clock::timestamp_ms(clock);
        if (feed.last_update_ms > 0) {
            let age = current_time - feed.last_update_ms;
            assert!(age <= feed.max_staleness_ms, E_PRICE_STALE);
        };

        (feed.price, feed.confidence, feed.last_update_ms)
    }

    /// Get price without any checks (USE WITH CAUTION)
    /// Only use this when you have your own validation logic
    /// or for display/informational purposes
    public fun get_price_unchecked(feed: &PriceFeed): u64 {
        feed.price
    }

    // =========================================================================
    // View Functions
    // =========================================================================

    /// Check if the price feed is currently valid (not paused, not stale)
    public fun is_valid(feed: &PriceFeed, clock: &Clock): bool {
        if (feed.paused) return false;
        if (feed.last_update_ms == 0) return true; // Initial price is valid

        let current_time = clock::timestamp_ms(clock);
        let age = current_time - feed.last_update_ms;

        age <= feed.max_staleness_ms
    }

    /// Get the age of the current price in milliseconds
    public fun get_price_age_ms(feed: &PriceFeed, clock: &Clock): u64 {
        if (feed.last_update_ms == 0) return 0;
        clock::timestamp_ms(clock) - feed.last_update_ms
    }

    /// Get the trading pair identifier
    public fun get_pair(feed: &PriceFeed): vector<u8> {
        feed.pair
    }

    /// Get the number of decimal places
    public fun get_decimals(feed: &PriceFeed): u8 {
        feed.decimals
    }

    /// Check if feed is paused
    public fun is_paused(feed: &PriceFeed): bool {
        feed.paused
    }

    /// Get the last update timestamp
    public fun get_last_update_ms(feed: &PriceFeed): u64 {
        feed.last_update_ms
    }

    /// Get the total update count
    public fun get_update_count(feed: &PriceFeed): u64 {
        feed.update_count
    }

    /// Get the maximum staleness threshold
    public fun get_max_staleness_ms(feed: &PriceFeed): u64 {
        feed.max_staleness_ms
    }

    /// Get the confidence interval
    public fun get_confidence(feed: &PriceFeed): u64 {
        feed.confidence
    }

    /// Get feed info (for backwards compatibility)
    public fun feed_info(feed: &PriceFeed): (vector<u8>, u64, u64, u64) {
        (feed.pair, feed.price, feed.confidence, feed.last_update_ms)
    }

    // =========================================================================
    // Admin Functions
    // =========================================================================

    /// Pause the price feed (emergency use)
    /// While paused, get_price() will revert
    public fun pause(feed: &mut PriceFeed, cap: &AdminCap) {
        assert!(cap.feed_id == object::id(feed), E_NOT_AUTHORIZED);
        feed.paused = true;

        event::emit(FeedPauseChanged {
            feed_id: object::id(feed),
            paused: true,
        });
    }

    /// Unpause the price feed
    public fun unpause(feed: &mut PriceFeed, cap: &AdminCap) {
        assert!(cap.feed_id == object::id(feed), E_NOT_AUTHORIZED);
        feed.paused = false;

        event::emit(FeedPauseChanged {
            feed_id: object::id(feed),
            paused: false,
        });
    }

    /// Update the maximum staleness threshold
    public fun set_max_staleness(
        feed: &mut PriceFeed,
        cap: &AdminCap,
        new_staleness_ms: u64
    ) {
        assert!(cap.feed_id == object::id(feed), E_NOT_AUTHORIZED);
        assert!(new_staleness_ms >= MIN_STALENESS_MS, E_INVALID_STALENESS);
        assert!(new_staleness_ms <= MAX_STALENESS_MS, E_INVALID_STALENESS);

        let old_staleness = feed.max_staleness_ms;
        feed.max_staleness_ms = new_staleness_ms;

        event::emit(StalenessChanged {
            feed_id: object::id(feed),
            old_staleness_ms: old_staleness,
            new_staleness_ms,
        });
    }

    /// Create a new updater capability (for delegation)
    public fun create_updater_cap(
        feed: &PriceFeed,
        cap: &AdminCap,
        ctx: &mut TxContext
    ): UpdaterCap {
        assert!(cap.feed_id == object::id(feed), E_NOT_AUTHORIZED);

        let updater_uid = object::new(ctx);
        let updater_cap_id = updater_uid.to_inner();

        event::emit(UpdaterCapCreated {
            feed_id: object::id(feed),
            updater_cap_id,
            created_by: ctx.sender(),
        });

        UpdaterCap {
            id: updater_uid,
            feed_id: object::id(feed),
        }
    }

    /// Revoke an updater capability by destroying it
    public fun revoke_updater_cap(cap: UpdaterCap) {
        let UpdaterCap { id, feed_id } = cap;

        event::emit(UpdaterCapRevoked {
            feed_id,
            updater_cap_id: object::uid_to_inner(&id),
        });

        object::delete(id);
    }

    // =========================================================================
    // Entry Functions for Testing
    // =========================================================================

    /// Entry: Pause the feed
    entry fun pause_entry(feed: &mut PriceFeed, cap: &AdminCap) {
        pause(feed, cap);
    }

    /// Entry: Unpause the feed
    entry fun unpause_entry(feed: &mut PriceFeed, cap: &AdminCap) {
        unpause(feed, cap);
    }

    /// Entry: Set max staleness
    entry fun set_max_staleness_entry(
        feed: &mut PriceFeed,
        cap: &AdminCap,
        new_staleness_ms: u64
    ) {
        set_max_staleness(feed, cap, new_staleness_ms);
    }

    /// Entry: Create and transfer updater cap
    entry fun create_updater_cap_entry(
        feed: &PriceFeed,
        cap: &AdminCap,
        recipient: address,
        ctx: &mut TxContext
    ) {
        let updater_cap = create_updater_cap(feed, cap, ctx);
        transfer::transfer(updater_cap, recipient);
    }
}
