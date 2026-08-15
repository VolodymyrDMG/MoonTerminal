//! Arbitrage overlay settings: whether and how other-exchange prices from the core's arbitrage
//! relay are drawn on charts — master switch, the bot-mirrored display flags (lines, legend
//! numbers, absolute prices, spread percent, right-side legend), and a per-platform enable +
//! color map keyed by the platform's display name ("BybitF", "GateS", "HL#1", …).
//!
//! Persistence is a SEPARATE portable `arb_view.toml` (like `detects_view.toml`): the relay data
//! is per core, but the way one trader wants it drawn is one global preference. A platform absent
//! from the map is ENABLED with a deterministic palette color, so a newly appearing exchange shows
//! up immediately and only an explicit uncheck hides it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{paths, toml_io};

/// Per-platform display preference for the arbitrage overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArbPlatformView {
    /// Draw this platform's price line and legend row.
    pub on: bool,
    /// Line and label color as RGB.
    pub color: [u8; 3],
}

impl Default for ArbPlatformView {
    fn default() -> Self {
        Self {
            on: true,
            color: [176, 122, 66],
        }
    }
}

/// Stable readable color for a platform name, used until the user picks their own.
///
/// A tiny FNV-style hash spreads hues; saturation and lightness stay in a band that reads on both
/// the dark and light chart themes. Deterministic: the same name gets the same color on every
/// machine, so shared screenshots stay comparable.
pub fn arb_default_color(name: &str) -> [u8; 3] {
    let mut h: u32 = 0x811c_9dc5;
    for b in name.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    // Map the hash to a hue in degrees and convert HSL(h, 0.62, 0.58) to RGB.
    let hue = f64::from(h % 360);
    let (s, l) = (0.62_f64, 0.58_f64);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match hue as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    ]
}

/// Arbitrage overlay configuration; field meanings mirror the bot's Arbitrage panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArbViewCfg {
    /// Master switch. Off by default: without the bot-side arbitrage license and platform set,
    /// there is nothing to draw, and charts must not grow an empty legend unasked.
    pub enabled: bool,
    /// Draw a horizontal price line per platform.
    pub lines: bool,
    /// Show the legend rows (platform name + figures).
    pub numbers: bool,
    /// Show the spread against our price in percent.
    pub percent: bool,
    /// Show the platform's absolute price next to the percent.
    pub prices: bool,
    /// Anchor the legend at the chart's right edge instead of the left.
    pub right: bool,
    /// Per-platform overrides keyed by display name; a missing entry means "enabled, palette
    /// color" (see [`ArbViewCfg::platform`]).
    pub platforms: BTreeMap<String, ArbPlatformView>,
}

impl Default for ArbViewCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            lines: true,
            numbers: true,
            percent: true,
            prices: false,
            right: false,
            platforms: BTreeMap::new(),
        }
    }
}

impl ArbViewCfg {
    /// Effective per-platform preference: the stored entry, or enabled with the palette color.
    pub fn platform(&self, name: &str) -> ArbPlatformView {
        self.platforms
            .get(name)
            .copied()
            .unwrap_or(ArbPlatformView {
                on: true,
                color: arb_default_color(name),
            })
    }

    /// Store an explicit per-platform preference.
    pub fn set_platform(&mut self, name: &str, view: ArbPlatformView) {
        self.platforms.insert(name.to_string(), view);
    }
}

/// `arb_view.toml` file wrapper.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArbViewFile {
    pub view: ArbViewCfg,
}

impl ArbViewFile {
    pub fn load() -> Self {
        toml_io::load_or_default(&paths::arb_view_path(), "arb_view.toml", |_| {})
    }

    pub fn save(&self) {
        if let Err(e) = toml_io::save(&paths::arb_view_path(), self, "arb_view.toml") {
            log::warn!("не записал arb_view.toml: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests;
