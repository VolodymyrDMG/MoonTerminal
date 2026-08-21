//! Chart volume-zone settings for the fork's half-second buy/sell band: the master switch and
//! the band height. Persistence is a SEPARATE portable `vol_view.toml` (like `arb_view.toml`):
//! the tick data is per pane, but how one trader wants volumes drawn is one global preference.
//! Everything here feeds `ChartDataState` through the render-settings chain, so a change applies
//! to every open chart on the next frame without reopening anything.

use serde::{Deserialize, Serialize};

use super::{paths, toml_io};

/// Volume-zone band heights offered by the gear popup, stored as a small integer.
pub const VOL_HEIGHT_S: u8 = 0;
pub const VOL_HEIGHT_M: u8 = 1;
pub const VOL_HEIGHT_L: u8 = 2;

/// Chart volume-zone configuration.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VolViewCfg {
    /// Master switch for the half-second volume band. On by default.
    pub enabled: bool,
    /// Band height: 0=S, 1=M, 2=L.
    pub height: u8,
}

impl Default for VolViewCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            height: VOL_HEIGHT_M,
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
