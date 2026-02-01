/// Arbitrage Module for Perp DEX and DeepBook V3 Integration
///
/// This module enables atomic arbitrage between:
/// - Perpetual DEX (uses oracle price)
/// - DeepBook V3 spot market (uses order book mid price)
///
/// # Arbitrage Strategy
/// When oracle price differs significantly from spot mid-price:
/// 1. If oracle_price > spot_price (oracle is high):
///    - Open SHORT on perp (betting price will fall to spot level)
///    - Buy on spot market (capture the lower price)
///    - Profit when prices converge
///
/// 2. If oracle_price < spot_price (oracle is low):
///    - Open LONG on perp (betting price will rise to spot level)
///    - Sell on spot market (capture the higher price)
///    - Profit when prices converge
///
/// # DeepBook V3 Integration
/// Uses the real DeepBook V3 package functions:
/// - `pool::mid_price()` for spot price reference
/// - `pool::place_market_order()` for spot execution
/// - `balance_manager` for fund management
///
/// # Single PTB Execution
/// All arbitrage operations execute atomically in one transaction.
#[allow(duplicate_alias, unused_const, unused_field, lint(self_transfer))]
module perp_dex::arbitrage {
    use sui::coin::{Self, Coin};
    use sui::sui::SUI;
    use sui::clock::Clock;
    use sui::event;
    use perp_dex::oracle::{Self, PriceFeed};
    use perp_dex::perpetual::{Self, PerpExchange, Position};
    use deepbook::pool::{Self, Pool};
    use deepbook::balance_manager::{Self, BalanceManager, TradeProof};

    // =========================================================================
    // Error Codes
    // =========================================================================

    /// Spread too small for profitable arbitrage
    const E_SPREAD_TOO_SMALL: u64 = 300;
    /// No liquidity in spot market
    const E_NO_SPOT_LIQUIDITY: u64 = 301;
    /// Insufficient funds for arbitrage
    const E_INSUFFICIENT_FUNDS: u64 = 302;
    /// Position size exceeds limit
    const E_SIZE_TOO_LARGE: u64 = 303;
    /// Arbitrage would not be profitable after fees
    const E_NOT_PROFITABLE: u64 = 304;
    /// Oracle price is zero
    const E_INVALID_ORACLE: u64 = 305;
    /// Spot price is zero
    const E_INVALID_SPOT: u64 = 306;

    // =========================================================================
    // Constants
    // =========================================================================

    /// Minimum spread in basis points to trigger arbitrage (50 bps = 0.5%)
    const MIN_SPREAD_BPS: u64 = 50;
    /// Basis points denominator
    const BPS_DENOMINATOR: u64 = 10000;
    /// Self-matching option: allowed
    const SELF_MATCHING_ALLOWED: u8 = 0;

    // =========================================================================
    // Structs
    // =========================================================================

    /// Tracks an arbitrage opportunity
    public struct ArbitrageOpportunity has copy, drop, store {
        /// Oracle price (8 decimals)
        oracle_price: u64,
        /// Spot mid price (8 decimals)
        spot_price: u64,
        /// Spread in basis points
        spread_bps: u64,
        /// True if oracle > spot (should short perp, buy spot)
        oracle_premium: bool,
        /// Estimated profit in basis points
        estimated_profit_bps: u64,
    }

    /// Result of an arbitrage execution
    public struct ArbitrageResult has copy, drop, store {
        /// Whether arbitrage was executed
        executed: bool,
        /// Perp position size
        perp_size: u64,
        /// Spot order size (base)
        spot_size: u64,
        /// Entry prices
        perp_entry_price: u64,
        spot_fill_price: u64,
        /// Estimated profit potential
        profit_potential_bps: u64,
    }

    // =========================================================================
    // Events
    // =========================================================================

    /// Emitted when an arbitrage opportunity is detected
    public struct ArbitrageOpportunityDetected has copy, drop {
        oracle_price: u64,
        spot_price: u64,
        spread_bps: u64,
        oracle_premium: bool,
    }

    /// Emitted when arbitrage is executed
    public struct ArbitrageExecuted has copy, drop {
        trader: address,
        oracle_price: u64,
        spot_price: u64,
        spread_bps: u64,
        perp_position_id: ID,
        perp_is_long: bool,
        perp_size: u64,
        spot_base_amount: u64,
        spot_quote_amount: u64,
    }

    // =========================================================================
    // Price Discovery using Real DeepBook V3
    // =========================================================================

    /// Check for arbitrage opportunity between oracle and DeepBook spot pool
    /// Uses DeepBook V3's mid_price function
    public fun check_opportunity<BaseAsset, QuoteAsset>(
        oracle: &PriceFeed,
        spot_pool: &Pool<BaseAsset, QuoteAsset>,
        clock: &Clock,
    ): ArbitrageOpportunity {
        let oracle_price = oracle::get_price_unchecked(oracle);
        let spot_price = pool::mid_price(spot_pool, clock);

        // Calculate spread
        let (spread_bps, oracle_premium) = if (oracle_price > spot_price && spot_price > 0) {
            let diff = oracle_price - spot_price;
            let spread = (diff * BPS_DENOMINATOR) / spot_price;
            (spread, true)
        } else if (spot_price > oracle_price && oracle_price > 0) {
            let diff = spot_price - oracle_price;
            let spread = (diff * BPS_DENOMINATOR) / oracle_price;
            (spread, false)
        } else {
            (0, false)
        };

        // Estimate profit after fees (assume ~20 bps total fees)
        let estimated_profit = if (spread_bps > 20) {
            spread_bps - 20
        } else {
            0
        };

        ArbitrageOpportunity {
            oracle_price,
            spot_price,
            spread_bps,
            oracle_premium,
            estimated_profit_bps: estimated_profit,
        }
    }

    /// Check if an opportunity is profitable
    public fun is_profitable(opp: &ArbitrageOpportunity): bool {
        opp.spread_bps >= MIN_SPREAD_BPS && opp.estimated_profit_bps > 0
    }

    /// Get current prices from both sources
    public fun get_prices<BaseAsset, QuoteAsset>(
        oracle: &PriceFeed,
        spot_pool: &Pool<BaseAsset, QuoteAsset>,
        clock: &Clock,
    ): (u64, u64, u64) {
        let oracle_price = oracle::get_price_unchecked(oracle);
        let spot_price = pool::mid_price(spot_pool, clock);

        let spread_bps = if (oracle_price > spot_price && spot_price > 0) {
            ((oracle_price - spot_price) * BPS_DENOMINATOR) / spot_price
        } else if (spot_price > oracle_price && oracle_price > 0) {
            ((spot_price - oracle_price) * BPS_DENOMINATOR) / oracle_price
        } else {
            0
        };

        (oracle_price, spot_price, spread_bps)
    }

    // =========================================================================
    // Arbitrage Execution with Real DeepBook V3
    // =========================================================================

    /// Execute atomic arbitrage in a single PTB using real DeepBook V3
    ///
    /// This function:
    /// 1. Checks prices and validates opportunity
    /// 2. Opens perp position (long or short based on price discrepancy)
    /// 3. Executes opposite spot trade on DeepBook
    ///
    /// # Type Parameters
    /// * `BaseAsset` - The base asset type for DeepBook pool
    /// * `QuoteAsset` - The quote asset type for DeepBook pool
    ///
    /// # Arguments
    /// * `perp_exchange` - The perpetual exchange
    /// * `oracle` - Price feed for perp entry
    /// * `spot_pool` - DeepBook V3 spot market pool
    /// * `balance_manager` - DeepBook balance manager for spot trading
    /// * `trade_proof` - Proof of trading authorization
    /// * `perp_collateral` - SUI for perp collateral
    /// * `leverage` - Leverage for perp position
    /// * `spot_quantity` - Quantity for spot order (in base asset terms)
    /// * `clock` - Clock for timestamp
    ///
    /// # Returns
    /// * Position - The opened perp position
    /// * ArbitrageResult - Details of the execution
    public fun execute_arbitrage<BaseAsset, QuoteAsset>(
        perp_exchange: &mut PerpExchange,
        oracle: &PriceFeed,
        spot_pool: &mut Pool<BaseAsset, QuoteAsset>,
        balance_manager: &mut BalanceManager,
        trade_proof: &TradeProof,
        perp_collateral: Coin<SUI>,
        leverage: u64,
        spot_quantity: u64,
        clock: &Clock,
        ctx: &mut TxContext
    ): (Position, ArbitrageResult) {
        // Get prices
        let oracle_price = oracle::get_price_unchecked(oracle);
        let spot_price = pool::mid_price(spot_pool, clock);

        assert!(oracle_price > 0, E_INVALID_ORACLE);
        assert!(spot_price > 0, E_INVALID_SPOT);

        // Check opportunity
        let opp = check_opportunity(oracle, spot_pool, clock);
        assert!(opp.spread_bps >= MIN_SPREAD_BPS, E_SPREAD_TOO_SMALL);

        // Determine direction
        // If oracle > spot: SHORT perp (expect price to fall), BUY spot
        // If oracle < spot: LONG perp (expect price to rise), SELL spot
        let perp_is_long = !opp.oracle_premium;

        // Calculate position sizes
        let collateral_value = coin::value(&perp_collateral);
        let perp_size = collateral_value * leverage;

        // Open perp position
        let position = perpetual::open_position(
            perp_exchange,
            oracle,
            perp_collateral,
            leverage,
            perp_is_long,
            ctx
        );

        // Execute spot trade on DeepBook V3
        // is_bid = true means buying (when oracle > spot, we buy spot)
        // is_bid = false means selling (when oracle < spot, we sell spot)
        let is_bid = opp.oracle_premium;

        let order_info = pool::place_market_order(
            spot_pool,
            balance_manager,
            trade_proof,
            0, // client_order_id
            SELF_MATCHING_ALLOWED,
            spot_quantity,
            is_bid,
            false, // pay_with_deep
            clock,
            ctx
        );

        // Get fill amounts from order info
        let spot_base = order_info.executed_quantity();
        let spot_quote = order_info.cumulative_quote_quantity();

        // Emit arbitrage event
        event::emit(ArbitrageExecuted {
            trader: ctx.sender(),
            oracle_price,
            spot_price,
            spread_bps: opp.spread_bps,
            perp_position_id: object::id(&position),
            perp_is_long,
            perp_size,
            spot_base_amount: spot_base,
            spot_quote_amount: spot_quote,
        });

        let result = ArbitrageResult {
            executed: true,
            perp_size,
            spot_size: spot_base,
            perp_entry_price: oracle_price,
            spot_fill_price: spot_price,
            profit_potential_bps: opp.estimated_profit_bps,
        };

        (position, result)
    }

    /// Execute perp-only arbitrage position based on price discrepancy
    /// Opens a perp position when there's a price discrepancy with DeepBook spot
    /// The spot hedge can be done separately or skipped
    public fun execute_perp_arbitrage<BaseAsset, QuoteAsset>(
        perp_exchange: &mut PerpExchange,
        oracle: &PriceFeed,
        spot_pool: &Pool<BaseAsset, QuoteAsset>,
        perp_collateral: Coin<SUI>,
        leverage: u64,
        clock: &Clock,
        ctx: &mut TxContext
    ): Position {
        // Get prices
        let oracle_price = oracle::get_price_unchecked(oracle);
        let spot_price = pool::mid_price(spot_pool, clock);

        assert!(oracle_price > 0, E_INVALID_ORACLE);
        assert!(spot_price > 0, E_INVALID_SPOT);

        // Check opportunity
        let opp = check_opportunity(oracle, spot_pool, clock);

        // Emit opportunity detection
        event::emit(ArbitrageOpportunityDetected {
            oracle_price: opp.oracle_price,
            spot_price: opp.spot_price,
            spread_bps: opp.spread_bps,
            oracle_premium: opp.oracle_premium,
        });

        // Determine direction based on price discrepancy
        // Short if oracle > spot (expect convergence down)
        // Long if oracle < spot (expect convergence up)
        let perp_is_long = !opp.oracle_premium;

        // Open perp position
        perpetual::open_position(
            perp_exchange,
            oracle,
            perp_collateral,
            leverage,
            perp_is_long,
            ctx
        )
    }

    // =========================================================================
    // View Functions
    // =========================================================================

    /// Get arbitrage opportunity details
    public fun opportunity_info(opp: &ArbitrageOpportunity): (u64, u64, u64, bool, u64) {
        (
            opp.oracle_price,
            opp.spot_price,
            opp.spread_bps,
            opp.oracle_premium,
            opp.estimated_profit_bps
        )
    }

    /// Get arbitrage result details
    public fun result_info(result: &ArbitrageResult): (bool, u64, u64, u64, u64, u64) {
        (
            result.executed,
            result.perp_size,
            result.spot_size,
            result.perp_entry_price,
            result.spot_fill_price,
            result.profit_potential_bps
        )
    }

    // =========================================================================
    // Helper Functions
    // =========================================================================

    /// Calculate optimal position size based on spread and risk parameters
    public fun calculate_position_size(
        available_capital: u64,
        spread_bps: u64,
        max_risk_pct: u64,
    ): u64 {
        // Size proportional to spread (more spread = larger size)
        // But capped at max_risk_pct of capital
        let base_size = (available_capital * spread_bps) / BPS_DENOMINATOR;
        let max_size = (available_capital * max_risk_pct) / 100;

        if (base_size > max_size) {
            max_size
        } else {
            base_size
        }
    }

    /// Estimate profit from arbitrage given position size and spread
    public fun estimate_profit(
        position_size: u64,
        spread_bps: u64,
        total_fee_bps: u64,
    ): (u64, bool) {
        if (spread_bps <= total_fee_bps) {
            // Would be a loss
            let loss = (position_size * (total_fee_bps - spread_bps)) / BPS_DENOMINATOR;
            (loss, false)
        } else {
            let profit = (position_size * (spread_bps - total_fee_bps)) / BPS_DENOMINATOR;
            (profit, true)
        }
    }

    // =========================================================================
    // Entry Functions
    // =========================================================================

    /// Entry: Execute full arbitrage with DeepBook V3
    ///
    /// Note: This entry function requires the caller to generate a TradeProof
    /// using balance_manager::generate_proof_as_owner() in the same PTB before
    /// calling this function. TradeProof is a hot-potato type and cannot be
    /// passed directly to entry functions.
    ///
    /// For PTB usage:
    /// 1. Call balance_manager::generate_proof_as_owner(bm) -> TradeProof
    /// 2. Call this function with the proof
    /// 3. The proof is consumed by the spot order
    public fun execute_arbitrage_with_proof<BaseAsset, QuoteAsset>(
        perp_exchange: &mut PerpExchange,
        oracle: &PriceFeed,
        spot_pool: &mut Pool<BaseAsset, QuoteAsset>,
        balance_manager: &mut BalanceManager,
        trade_proof: &TradeProof,
        perp_collateral: Coin<SUI>,
        leverage: u64,
        spot_quantity: u64,
        clock: &Clock,
        ctx: &mut TxContext
    ): Position {
        let (position, _result) = execute_arbitrage(
            perp_exchange,
            oracle,
            spot_pool,
            balance_manager,
            trade_proof,
            perp_collateral,
            leverage,
            spot_quantity,
            clock,
            ctx
        );
        position
    }

    /// Entry: Execute perp-only arbitrage with DeepBook price reference
    entry fun execute_perp_arbitrage_entry<BaseAsset, QuoteAsset>(
        perp_exchange: &mut PerpExchange,
        oracle: &PriceFeed,
        spot_pool: &Pool<BaseAsset, QuoteAsset>,
        perp_collateral: Coin<SUI>,
        leverage: u64,
        clock: &Clock,
        ctx: &mut TxContext
    ) {
        let position = execute_perp_arbitrage(
            perp_exchange,
            oracle,
            spot_pool,
            perp_collateral,
            leverage,
            clock,
            ctx
        );
        transfer::public_transfer(position, ctx.sender());
    }
}
