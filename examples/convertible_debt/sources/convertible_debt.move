module convertible_debt::convertible_debt {
    use sui::balance::{Self, Balance};
    use sui::coin::{Self, Coin};
    use sui::object;
    use sui::object::UID;
    use sui::transfer;
    use sui::tx_context::{Self, TxContext};

    use convertible_debt::oracle;
    use convertible_debt::oracle::Oracle;
    use convertible_debt::tokens::{ETH, USD};

    const BASIS_POINTS: u64 = 10_000;

    const E_BAD_YIELD: u64 = 0;
    const E_BAD_PAYMENT: u64 = 1;
    const E_BAD_COLLATERAL: u64 = 2;
    const E_OFFER_CLOSED: u64 = 3;
    const E_NOT_BORROWER: u64 = 4;
    const E_NOT_LENDER: u64 = 5;
    const E_NOT_MATURE: u64 = 6;
    const E_INSUFFICIENT_REPAY: u64 = 7;
    const E_ALREADY_SETTLED: u64 = 8;
    const E_BAD_STRIKE: u64 = 9;

    /// Borrower listing with locked ETH collateral.
    public struct Offer has key, store {
        id: UID,
        borrower: address,
        collateral: Balance<ETH>,
        principal: u64,
        yield_bps: u64,
        maturity_ms: u64,
        strike_price: u64,
        is_open: bool,
    }

    /// Shared note with lender/borrower rights.
    public struct Note has key, store {
        id: UID,
        borrower: address,
        lender: address,
        collateral: Balance<ETH>,
        repay_balance: Balance<USD>,
        principal: u64,
        yield_bps: u64,
        maturity_ms: u64,
        strike_price: u64,
        settled: bool,
    }

    /// Total owed at maturity (principal + fixed yield).
    public fun owed_amount(principal: u64, yield_bps: u64): u64 {
        let principal_128 = principal as u128;
        let yield_amount = principal_128 * (yield_bps as u128) / (BASIS_POINTS as u128);
        (principal_128 + yield_amount) as u64
    }

    /// Required ETH collateral at strike price.
    public fun required_collateral(principal: u64, yield_bps: u64, strike_price: u64): u64 {
        let owed = owed_amount(principal, yield_bps);
        let scale = oracle::price_scale();
        let numerator = (owed as u128) * (scale as u128);
        (numerator / (strike_price as u128)) as u64
    }

    /// Create and share a convertible offer.
    public entry fun create_offer(
        collateral: Coin<ETH>,
        principal: u64,
        yield_bps: u64,
        maturity_ms: u64,
        oracle: &Oracle,
        ctx: &mut TxContext,
    ) {
        assert!(yield_bps <= BASIS_POINTS, E_BAD_YIELD);
        let strike = oracle::get_price(oracle);
        assert!(strike > 0, E_BAD_STRIKE);

        let required = required_collateral(principal, yield_bps, strike);
        assert!(coin::value(&collateral) >= required, E_BAD_COLLATERAL);

        let offer = Offer {
            id: object::new(ctx),
            borrower: tx_context::sender(ctx),
            collateral: coin::into_balance(collateral),
            principal,
            yield_bps,
            maturity_ms,
            strike_price: strike,
            is_open: true,
        };

        transfer::share_object(offer);
    }

    /// Cancel an open offer and return collateral to the borrower.
    public entry fun cancel_offer(offer: &mut Offer, ctx: &mut TxContext) {
        assert!(offer.is_open, E_OFFER_CLOSED);
        assert!(tx_context::sender(ctx) == offer.borrower, E_NOT_BORROWER);

        let collateral = balance::withdraw_all(&mut offer.collateral);
        offer.is_open = false;
        transfer::public_transfer(coin::from_balance(collateral, ctx), offer.borrower);
    }

    /// Accept an offer and mint a shared convertible note.
    public entry fun take_offer(offer: &mut Offer, payment: Coin<USD>, ctx: &mut TxContext) {
        assert!(offer.is_open, E_OFFER_CLOSED);
        let payment_value = coin::value(&payment);
        assert!(payment_value == offer.principal, E_BAD_PAYMENT);

        transfer::public_transfer(payment, offer.borrower);

        let collateral = balance::withdraw_all(&mut offer.collateral);
        offer.is_open = false;

        let note = Note {
            id: object::new(ctx),
            borrower: offer.borrower,
            lender: tx_context::sender(ctx),
            collateral,
            repay_balance: balance::zero<USD>(),
            principal: offer.principal,
            yield_bps: offer.yield_bps,
            maturity_ms: offer.maturity_ms,
            strike_price: offer.strike_price,
            settled: false,
        };

        transfer::share_object(note);
    }

    /// Borrower repays in USD into the shared note.
    public entry fun repay(note: &mut Note, repayment: Coin<USD>, ctx: &mut TxContext) {
        assert!(!note.settled, E_ALREADY_SETTLED);
        assert!(tx_context::sender(ctx) == note.borrower, E_NOT_BORROWER);

        let repay_balance = coin::into_balance(repayment);
        balance::join(&mut note.repay_balance, repay_balance);
    }

    /// Lender redeems USD repayment at or after maturity.
    public entry fun redeem(note: &mut Note, now_ms: u64, ctx: &mut TxContext) {
        assert!(!note.settled, E_ALREADY_SETTLED);
        assert!(tx_context::sender(ctx) == note.lender, E_NOT_LENDER);
        assert!(now_ms >= note.maturity_ms, E_NOT_MATURE);

        let owed = owed_amount(note.principal, note.yield_bps);
        let available = balance::value(&note.repay_balance);
        assert!(available >= owed, E_INSUFFICIENT_REPAY);

        let payout = balance::split(&mut note.repay_balance, owed);
        transfer::public_transfer(coin::from_balance(payout, ctx), note.lender);

        let remaining = balance::withdraw_all(&mut note.repay_balance);
        if (balance::value(&remaining) > 0) {
            transfer::public_transfer(coin::from_balance(remaining, ctx), note.borrower);
        } else {
            balance::destroy_zero(remaining);
        };

        let collateral = balance::withdraw_all(&mut note.collateral);
        transfer::public_transfer(coin::from_balance(collateral, ctx), note.borrower);

        note.settled = true;
    }

    /// Lender converts the note to ETH at the original strike price.
    /// No maturity check to allow early conversion.
    public entry fun convert(note: &mut Note, ctx: &mut TxContext) {
        assert!(!note.settled, E_ALREADY_SETTLED);
        assert!(tx_context::sender(ctx) == note.lender, E_NOT_LENDER);

        let owed = owed_amount(note.principal, note.yield_bps);
        let scale = oracle::price_scale();
        let numerator = (owed as u128) * (scale as u128);
        let eth_out = (numerator / (note.strike_price as u128)) as u64;

        let available = balance::value(&note.collateral);
        assert!(available >= eth_out, E_BAD_COLLATERAL);

        let payout = balance::split(&mut note.collateral, eth_out);
        transfer::public_transfer(coin::from_balance(payout, ctx), note.lender);

        let remaining_collateral = balance::withdraw_all(&mut note.collateral);
        if (balance::value(&remaining_collateral) > 0) {
            transfer::public_transfer(coin::from_balance(remaining_collateral, ctx), note.borrower);
        } else {
            balance::destroy_zero(remaining_collateral);
        };

        let remaining_repay = balance::withdraw_all(&mut note.repay_balance);
        if (balance::value(&remaining_repay) > 0) {
            transfer::public_transfer(coin::from_balance(remaining_repay, ctx), note.borrower);
        } else {
            balance::destroy_zero(remaining_repay);
        };

        note.settled = true;
    }
}
