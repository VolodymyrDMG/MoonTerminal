//! Arbitrage relay → UI rows: turn per-market moonproto arb slots into plain [`ArbQuote`]s.
//!
//! The core relays other-exchange prices only for the platforms enabled in the BOT's arbitrage
//! panel (and only while its arbitrage license is active); moonproto has already applied them to
//! retained market state by the time this module runs. Collection is therefore a read-only
//! sweep: probe every known platform slot on every market handle and keep what exists. The
//! platform table below is the protocol's fixed code space with the BOT's display spellings
//! ("BybitF", "GateS", …), so the terminal legend reads like the bot's. HIP-3 deployer slots
//! (codes 50+) have no name channel in the relay yet and are labeled "HL#n".

use moonproto::{ArbPlatformCode, MarketArbSlot, MoonStateSnapshot};

use crate::feed::types::ArbQuote;

/// Fixed platform code space with bot-style display names; the byte mirrors the protocol code so
/// consumers can match a quote to a connected core's venue without moonproto types.
const PLATFORMS: &[(ArbPlatformCode, &str, u8)] = &[
    (ArbPlatformCode::FBybit, "BybitF", 2),
    (ArbPlatformCode::Binance, "BinanceS", 3),
    (ArbPlatformCode::FBinance, "BinanceF", 4),
    (ArbPlatformCode::Huobi, "HtxS", 5),
    (ArbPlatformCode::QBinance, "BinanceQ", 6),
    (ArbPlatformCode::ByBit, "BybitS", 7),
    (ArbPlatformCode::Gate, "GateS", 8),
    (ArbPlatformCode::FGate, "GateF", 9),
    (ArbPlatformCode::BitGet, "BitgetS", 10),
    (ArbPlatformCode::FBitGet, "BitgetF", 11),
    (ArbPlatformCode::HyperSpot, "HL_S", 12),
    (ArbPlatformCode::HyperFutures, "HL_F", 13),
    (ArbPlatformCode::Forex, "Forex", 100),
    (ArbPlatformCode::UpBit, "UpBit", 101),
    (ArbPlatformCode::Okx, "OkxS", 102),
    (ArbPlatformCode::BinAlpha, "BinAlpha", 103),
];

/// How many HIP-3 deployer slots to probe (codes 50..50+N). The bot UI currently shows four;
/// twelve leaves headroom without turning the sweep into a scan of the whole byte space.
const HL_DEPLOYER_SLOTS: u8 = 12;

/// Sweep every market's arb slots into `(market, quotes)` rows; markets without data are absent.
pub(super) fn collect_arb(snap: &MoonStateSnapshot) -> Vec<(String, Vec<ArbQuote>)> {
    let mut out = Vec::new();
    for handle in snap.markets().iter() {
        let mut quotes: Vec<ArbQuote> = Vec::new();
        for (code, name, byte) in PLATFORMS {
            if let Some(slot) = handle.arb_slot(*code) {
                push_quote(&mut quotes, name, *byte, &slot);
            }
        }
        for i in 0..HL_DEPLOYER_SLOTS {
            if let Some(slot) = handle.arb_slot(ArbPlatformCode::hyper_deployer(i)) {
                push_quote(&mut quotes, &format!("HL#{}", i + 1), 50 + i, &slot);
            }
        }
        if !quotes.is_empty() {
            out.push((handle.name().to_string(), quotes));
        }
    }
    out
}

/// Convert one slot into a quote, skipping slots that never received a price.
fn push_quote(quotes: &mut Vec<ArbQuote>, name: &str, byte: u8, slot: &MarketArbSlot) {
    if !(slot.now.price > 0.0) {
        return;
    }
    let latest = slot.latest_point();
    let trail: Vec<(i64, f32, f32)> = slot
        .points_oldest_first()
        .iter()
        .filter(|p| p.price > 0.0)
        .map(|p| (p.unix_millis().unwrap_or(0), p.price, p.my_price))
        .collect();
    quotes.push(ArbQuote {
        platform: byte,
        platform_name: name.to_string(),
        price: slot.now.price,
        time_ms: slot.now.unix_millis().unwrap_or(0),
        my_price: latest.my_price,
        trail,
    });
}
