//! Chart volume-zone settings: the Moonbot-style time-volume graph at the bottom of every
//! chart pane and the header Bv/Sv readout that sums the same tick tape.
//!
//! Persistence is a SEPARATE portable `vol_view.toml` (like `arb_view.toml`): the tick data is
//! per pane, but how one trader wants volumes drawn is one global preference. Everything here
//! feeds `ChartDataState` through the render-settings chain, so a change applies to every open
//! chart on the next frame without reopening anything.

use serde::{Deserialize, Serialize};

use super::{paths, toml_io};

/// Volume-zone band heights offered by the gear popup, stored as a small integer.
pub const VOL_HEIGHT_S: u8 = 0;
pub const VOL_HEIGHT_M: u8 = 1;
pub const VOL_HEIGHT_L: u8 = 2;

/// Header-readout placement slots: two bands (top edge of the plot, bottom edge above the
/// volume zone) × three anchors, plus "hidden". Stored as a small integer so the TOML stays
/// hand-editable; every out-of-range byte reads as hidden rather than as a random corner.
pub const VOL_POS_TOP_LEFT: u8 = 0;
pub const VOL_POS_TOP_CENTER: u8 = 1;
pub const VOL_POS_TOP_RIGHT: u8 = 2;
pub const VOL_POS_BOTTOM_LEFT: u8 = 3;
pub const VOL_POS_BOTTOM_CENTER: u8 = 4;
pub const VOL_POS_BOTTOM_RIGHT: u8 = 5;
pub const VOL_POS_HIDDEN: u8 = 6;

/// Chart volume-zone configuration.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VolViewCfg {
    /// Master switch for the volume graph band. On by default: the band predates this setting,
    /// so a file from before the switch existed must keep drawing it.
    pub enabled: bool,
    /// Band height: 0=S, 1=M (the pre-setting look), 2=L.
    pub height: u8,
    /// Draw the cumulative volume delta (CVD) step line over the band: Σ(buy − sell) quote
    /// volume, anchored at the oldest retained tick, normalized into the band per visible window.
    pub cvd: bool,
    /// Header Bv/Sv window in seconds; the header menu offers 10s/30s/1m/5m/15m/1h.
    pub window_secs: u16,
    /// Placement slot of the Bv/Sv block (window chip, figures, net delta, pressure bar) —
    /// one of the `VOL_POS_*` values. Defaults to top-center: the plot's top-LEFT already
    /// carries the canvas caption plates ("Сделки/ордера"), which taught us that lesson.
    pub pos_vol: u8,
    /// Placement slot of the 24h-delta chip.
    pub pos_delta: u8,
    /// Placement slot of the session-profit ("Ses") chip.
    pub pos_ses: u8,
}

impl Default for VolViewCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            height: VOL_HEIGHT_M,
            cvd: true,
            window_secs: 60,
            pos_vol: VOL_POS_TOP_CENTER,
            pos_delta: VOL_POS_TOP_RIGHT,
            pos_ses: VOL_POS_TOP_RIGHT,
        }
    }
}

impl VolViewCfg {
    /// Fraction of the pane height the band may occupy for the configured height.
    pub fn band_fraction(&self) -> f32 {
        match self.height {
            VOL_HEIGHT_S => 0.12,
            VOL_HEIGHT_L => 0.32,
            _ => 0.22,
        }
    }

    /// Hard cap of the band height in pixels for the configured height.
    pub fn band_max_px(&self) -> f32 {
        match self.height {
            VOL_HEIGHT_S => 160.0,
            VOL_HEIGHT_L => 380.0,
            _ => 260.0,
        }
    }

    /// Header Bv/Sv window clamped to 5s..=1h; `0` (a hand-edited file) reads as the default.
    pub fn window_secs_clamped(&self) -> u32 {
        match self.window_secs {
            0 => 60,
            s => u32::from(s.clamp(5, 3600)),
        }
    }

    /// Normalize a stored placement byte: anything past the last real slot reads as hidden.
    pub fn pos_clamped(pos: u8) -> u8 {
        pos.min(VOL_POS_HIDDEN)
    }
}

/// `vol_view.toml` file wrapper.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VolViewFile {
    pub view: VolViewCfg,
}

impl VolViewFile {
    pub fn load() -> Self {
        toml_io::load_or_default(&paths::vol_view_path(), "vol_view.toml", |_| {})
    }

    pub fn save(&self) {
        if let Err(e) = toml_io::save(&paths::vol_view_path(), self, "vol_view.toml") {
            log::warn!("не записал vol_view.toml: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests;
