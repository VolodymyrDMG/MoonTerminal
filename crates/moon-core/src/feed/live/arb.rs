//! Arbitrage relay → UI rows: turn per-market moonproto arb slots into plain [`ArbQuote`]s.
//!
//! The core relays other-exchange prices only for the platforms enabled in the BOT's arbitrage
//! panel (and only while its arbitrage license is active); moonproto has already applied them to
//! retained market state by the time this module runs. Collection is therefore a read-only
//! sweep — and it probes the WHOLE code byte space, not just the names this build knows: the
//! bot grows new platforms (OkxS arrived under a code moonproto's table had not named yet), and
//! a slot with data must reach the screen even when all we can print is "P104". Known codes get
//! the bot's display spellings ("BybitF", "GateS", …); HIP-3 deployer slots are "HL#n".

use moonproto::{ArbPlatformCode, MarketArbSlot, MoonStateSnapshot};

use crate::feed::types::ArbQuote;

/// Display names for the codes this build knows, mirroring the bot's panel spellings.
fn platform_name(byte: u8) -> String {
    match byte {
        2 => "BybitF".to_string(),
        3 => "BinanceS".to_string(),
        4 => "BinanceF".to_string(),
        5 => "HtxS".to_string(),
        6 => "BinanceQ".to_string(),
        7 => "BybitS".to_string(),
        8 => "GateS".to_string(),
        9 => "GateF".to_string(),
        10 => "BitgetS".to_string(),
        11 => "BitgetF".to_string(),
        12 => "HL_S".to_string(),
        13 => "HL_F".to_string(),
        50..=61 => format!("HL#{}", byte - 49),
        100 => "Forex".to_string(),
        101 => "UpBit".to_string(),
        102 => "OkxF".to_string(),
        103 => "BinAlpha".to_string(),
        104 => "OkxS".to_string(),
        _ => format!("P{byte}"),
    }
}

/// Highest platform code probed; the known table tops out at 104 and deployers at 61, so 139
/// leaves generous headroom without scanning the whole byte space on every sweep.
const MAX_PLATFORM_CODE: u8 = 139;

/// Construct a platform code for ANY byte through the deployer constructor's wrapping offset —
/// the only public byte-taking constructor moonproto exposes.
fn code_of(byte: u8) -> ArbPlatformCode {
    ArbPlatformCode::hyper_deployer(byte.wrapping_sub(ArbPlatformCode::HL_DEX_BASE))
}

/// Sweep every market's arb slots into `(market, quotes)` rows; markets without data are absent.
pub(super) fn collect_arb(snap: &MoonStateSnapshot) -> Vec<(String, Vec<ArbQuote>)> {
    let mut out = Vec::new();
    for handle in snap.markets().iter() {
        let mut quotes: Vec<ArbQuote> = Vec::new();
        for byte in 1..=MAX_PLATFORM_CODE {
            if let Some(slot) = handle.arb_slot(code_of(byte)) {
                push_quote(&mut quotes, byte, &slot);
            }
        }
        if !quotes.is_empty() {
            out.push((handle.name().to_string(), quotes));
        }
    }
    out
}

/// Convert one slot into a quote, skipping slots that never received a price.
fn push_quote(quotes: &mut Vec<ArbQuote>, byte: u8, slot: &MarketArbSlot) {
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
        platform_name: platform_name(byte),
        price: slot.now.price,
        time_ms: slot.now.unix_millis().unwrap_or(0),
        my_price: latest.my_price,
        trail,
    });
}
