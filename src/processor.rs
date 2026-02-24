use crate::payload::DexSwapTx;

/// Simulated AMM pool state (pre-allocated, never heap-allocated).
/// Models a Uniswap v2 / Raydium-style constant-product pool: x * y = k.
#[repr(align(64))]
#[derive(Clone, Copy, Debug)]
pub struct AmmPoolState {
    /// Reserve of token0 (e.g. ETH/SOL) in the pool.
    pub reserve0: u64,
    /// Reserve of token1 (e.g. USDC) in the pool.
    pub reserve1: u64,
    /// Fee numerator (e.g. 3 for 0.3%).
    pub fee_num: u64,
    /// Fee denominator (e.g. 1000).
    pub fee_den: u64,
}

impl AmmPoolState {
    /// Constant-product AMM output calculation (no heap, no floats).
    ///
    /// Formula: amount_out = (reserve_out * amount_in_with_fee) / (reserve_in * fee_den + amount_in_with_fee)
    ///
    /// Returns `None` if reserves are zero or result would be zero.
    #[inline(always)]
    pub fn get_amount_out(&self, amount_in: u64, zero_for_one: bool) -> Option<u64> {
        let (reserve_in, reserve_out) = if zero_for_one {
            (self.reserve0, self.reserve1)
        } else {
            (self.reserve1, self.reserve0)
        };
        if reserve_in == 0 || reserve_out == 0 || amount_in == 0 {
            return None;
        }
        // amount_in_with_fee = amount_in * (fee_den - fee_num)
        let fee_adj = self.fee_den.checked_sub(self.fee_num)?;
        let amount_in_with_fee = amount_in.checked_mul(fee_adj)?;
        // numerator = reserve_out * amount_in_with_fee
        let numerator = (reserve_out as u128).checked_mul(amount_in_with_fee as u128)?;
        // denominator = reserve_in * fee_den + amount_in_with_fee
        let denominator = (reserve_in as u128)
            .checked_mul(self.fee_den as u128)?
            .checked_add(amount_in_with_fee as u128)?;
        let out = (numerator / denominator) as u64;
        if out == 0 { None } else { Some(out) }
    }

    /// Compute sandwich arbitrage profit (no allocations).
    ///
    /// Sandwich: we front-run the victim swap (buy token1 before victim),
    /// victim executes at worse price, we back-run (sell token1 back).
    /// Returns estimated profit in token0 units, or None if unprofitable.
    #[inline(always)]
    pub fn sandwich_profit(&self, victim_amount_in: u64, our_amount_in: u64, zero_for_one: bool) -> Option<u64> {
        // Step 1: our front-run buy (we buy token1 with our_amount_in of token0)
        let our_out = self.get_amount_out(our_amount_in, zero_for_one)?;
        // New pool reserves after our front-run
        let (new_reserve0, new_reserve1) = if zero_for_one {
            (self.reserve0.checked_add(our_amount_in)?, self.reserve1.checked_sub(our_out)?)
        } else {
            (self.reserve0.checked_sub(our_out)?, self.reserve1.checked_add(our_amount_in)?)
        };
        let pool_after_frontrun = AmmPoolState {
            reserve0: new_reserve0,
            reserve1: new_reserve1,
            fee_num: self.fee_num,
            fee_den: self.fee_den,
        };
        // Step 2: victim swap (victim buys in same direction, moving price further)
        let _ = pool_after_frontrun.get_amount_out(victim_amount_in, zero_for_one)?;
        let (r0_after_victim, r1_after_victim) = if zero_for_one {
            let victim_out = pool_after_frontrun.get_amount_out(victim_amount_in, zero_for_one)?;
            (new_reserve0.checked_add(victim_amount_in)?, new_reserve1.checked_sub(victim_out)?)
        } else {
            let victim_out = pool_after_frontrun.get_amount_out(victim_amount_in, zero_for_one)?;
            (new_reserve0.checked_sub(victim_out)?, new_reserve1.checked_add(victim_amount_in)?)
        };
        // Step 3: our back-run sell (we sell our_out of token1 back for token0)
        let pool_after_victim = AmmPoolState {
            reserve0: r0_after_victim,
            reserve1: r1_after_victim,
            fee_num: self.fee_num,
            fee_den: self.fee_den,
        };
        let back_run_out = pool_after_victim.get_amount_out(our_out, !zero_for_one)?;
        // Profit = what we get back minus what we put in
        back_run_out.checked_sub(our_amount_in)
    }
}

/// Static mock pool state — represents a Uniswap-style pool seeded with liquidity.
/// In production this would be updated from on-chain state reads.
static MOCK_POOL: AmmPoolState = AmmPoolState {
    reserve0: 1_000_000_000_000, // 1,000,000 token0 (e.g., 1M USDC, 6 decimals)
    reserve1: 500_000_000_000,   // 500,000 token1 (e.g., 500K ETH units)
    fee_num: 3,
    fee_den: 1_000,
};

/// Minimum profitable swap size — below this threshold, gas cost exceeds profit.
const MIN_AMOUNT_IN: u64 = 1_000_000;

/// Our front-run capital: fixed pre-allocated amount, no dynamic allocation.
const OUR_FRONT_RUN_AMOUNT: u64 = 10_000_000;

/// The hot-path processing logic: zero heap allocations.
///
/// Receives a raw wire payload, casts it to `DexSwapTx` via bytemuck (zero-copy),
/// evaluates the sandwich arbitrage opportunity using AMM constant-product math,
/// and returns the estimated profit in token0 units.
#[inline(always)]
pub fn process_packet(data: &[u8]) -> Option<u64> {
    let wire = data.get(..DexSwapTx::WIRE_SIZE)?;
    // Zero-copy cast: no allocation, no parsing loop — just a pointer reinterpretation.
    let tx = bytemuck::try_from_bytes::<DexSwapTx>(wire).ok()?;

    let amount_in = tx.amount_in();
    if amount_in < MIN_AMOUNT_IN {
        return None;
    }

    // direction: 0 = token0->token1, 1 = token1->token0
    let zero_for_one = tx.token_direction == 0;

    // Check slippage guard: victim's min_amount_out vs actual AMM output
    let victim_actual_out = MOCK_POOL.get_amount_out(amount_in, zero_for_one)?;
    if victim_actual_out < tx.min_amount_out() {
        // Victim tx would revert — not a valid sandwich target
        return None;
    }

    // Compute sandwich profit using constant-product AMM formula
    MOCK_POOL.sandwich_profit(amount_in, OUR_FRONT_RUN_AMOUNT, zero_for_one)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::DexSwapTx;
    use bytemuck::bytes_of;

    #[test]
    fn amm_get_amount_out_basic() {
        let pool = AmmPoolState { reserve0: 1_000_000, reserve1: 1_000_000, fee_num: 3, fee_den: 1_000 };
        let out = pool.get_amount_out(1_000, true).expect("should produce output");
        // With equal reserves and small input, output should be slightly less than input (fee + slippage)
        assert!(out < 1_000);
        assert!(out > 900);
    }

    #[test]
    fn amm_rejects_zero_reserves() {
        let pool = AmmPoolState { reserve0: 0, reserve1: 1_000_000, fee_num: 3, fee_den: 1_000 };
        assert!(pool.get_amount_out(1_000, true).is_none());
    }

    #[test]
    fn process_packet_profitable_swap() {
        let tx = DexSwapTx::from_parts(
            42,
            [0xAB; 20],
            50_000_000,  // large victim swap
            1,           // min_out = 1, so no slippage revert
            0,           // zero_for_one
        );
        let raw = bytes_of(&tx);
        let profit = process_packet(raw);
        assert!(profit.is_some(), "large swap should yield sandwich profit");
    }

    #[test]
    fn process_packet_rejects_small_swap() {
        let tx = DexSwapTx::from_parts(1, [0u8; 20], 500, 1, 0);
        let raw = bytes_of(&tx);
        assert!(process_packet(raw).is_none(), "below MIN_AMOUNT_IN should return None");
    }
}
