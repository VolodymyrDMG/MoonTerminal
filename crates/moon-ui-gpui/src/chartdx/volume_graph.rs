//! Moonbot-style time-volume graph: per-tick buy/sell quote volume accumulated into
//! screen-space columns for the existing per-instance volume pass.
//!
//! The old volume pass drew one hairline bar per trade cross with a square-root scale, which
//! reads as noise on liquid markets. Moonbot draws the raw tick stream instead: every trade
//! contributes its quote volume at its own time, with no time-grid aggregation — its "Time, sec"
//! setting only sizes the hover measure tool. This module owns the platform-free part of that
//! look: a retained per-pane tick tape fed from the same batches the combo ring consumes, and a
//! view-dependent accumulator that sums the tape into one column per couple of screen pixels.
//! Column sums ARE the per-tick structure at the resolution the screen can resolve: zoomed in, a
//! column is a single trade; zoomed out, it is exactly what those pixels cover.
//!
//! Only the aggregation lives here; backends draw the emitted columns as a live layer.

// The live consumer is the Metal layer for now; on other platforms the resample chain is
// intentionally dormant until their ports land, so its items would read as dead there.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use moon_core::feed::{Side, Tick};

use super::types::ChartCross;

/// Horizontal distance between emitted columns in pane pixels.
///
/// The volume shader draws each column 2.75 px wide, so a 2 px step keeps neighbouring columns
/// overlapping into a solid stepped area at every zoom level.
pub const COLUMN_STEP_PX: f32 = 2.0;

/// Opacity of the volume graph layer; bright enough for isolated per-trade needles on the dark
/// theme, still translucent where dense flow overlaps.
pub const GRAPH_ALPHA: f32 = 0.75;

/// Faint plate under the whole graph band, Moonbot's separate Vol-zone look.
pub const BAND_UNDERLAY_RGBA: [f32; 4] = [1.0, 1.0, 1.0, 0.045];

/// Hairline top edge of the graph band, separating it from the chart above.
pub const BAND_EDGE_RGBA: [f32; 4] = [1.0, 1.0, 1.0, 0.10];

/// Fraction of the pane height the graph band may occupy, mirrored by the volume shader.
pub const BAND_FRACTION: f32 = 0.22;

/// Hard cap of the band height in pixels, mirrored by the volume shader.
pub const BAND_MAX_PX: f32 = 260.0;

/// Retained tape length in trades; the oldest are pruned on ingest. Sized above the combo
/// ring's trade capacity so the graph never runs out before the crosses do.
const RETAIN_TICKS: usize = 400_000;

/// Extra window fraction accumulated on each side so panning inside the margin needs no rebuild.
const RESAMPLE_MARGIN: f32 = 0.25;

/// Returns the graph band height in device pixels for a pane of height `pane_h`.
///
/// Text labels position against this, so it must stay equal to the shader's band expression.
pub fn band_height_px(pane_h: f32) -> f32 {
    (pane_h * BAND_FRACTION).min(BAND_MAX_PX)
}

/// One retained trade of the volume tape: absolute time, quote volume, and side.
#[derive(Clone, Copy)]
struct VolTick {
    time_ms: f64,
    quote: f32,
    sell: bool,
}

/// Retained per-pane tick tape feeding the volume graph.
///
/// The tape keeps absolute times, so it is independent of the pane's render epoch and survives
/// zooming untouched. `revision` increments on every content change and keys the resample cache.
#[derive(Default)]
pub struct VolumeTape {
    ticks: Vec<VolTick>,
    /// True while a batch arrived with out-of-order times; sorted lazily before accumulation.
    unsorted: bool,
    pub revision: u64,
}

impl VolumeTape {
    /// Drops the tape, for market switches and full history resets.
    pub fn reset(&mut self) {
        if !self.ticks.is_empty() {
            self.ticks.clear();
        }
        self.unsorted = false;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Appends a tick batch and prunes the tape beyond `RETAIN_TICKS`.
    ///
    /// Quantities are in base units, so each trade contributes `qty * price` quote units.
    /// Non-finite and non-positive contributions are skipped: one corrupt tick must not poison
    /// the graph. Batches normally arrive time-ordered; a stray unordered row only marks the
    /// tape for a lazy sort instead of forcing one per batch.
    pub fn ingest(&mut self, ticks: &[Tick]) {
        if ticks.is_empty() {
            return;
        }
        let mut changed = false;
        for tick in ticks {
            let quote = tick.qty * tick.price;
            if !quote.is_finite() || quote <= 0.0 || !tick.time_ms.is_finite() {
                continue;
            }
            if let Some(last) = self.ticks.last() {
                if tick.time_ms < last.time_ms {
                    self.unsorted = true;
                }
            }
            self.ticks.push(VolTick {
                time_ms: tick.time_ms,
                quote,
                sell: tick.side == Side::Sell,
            });
            changed = true;
        }
        if changed {
            if self.ticks.len() > RETAIN_TICKS {
                let drop = self.ticks.len() - RETAIN_TICKS;
                self.ticks.drain(..drop);
            }
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// True when the tape holds no trades.
    pub fn is_empty(&self) -> bool {
        self.ticks.is_empty()
    }

    /// Sorts the tape if a batch arrived out of order, then returns the slice covering
    /// `[from_ms, to_ms)` by binary search.
    fn range(&mut self, from_ms: f64, to_ms: f64) -> &[VolTick] {
        if self.unsorted {
            self.ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
            self.unsorted = false;
        }
        let lo = self.ticks.partition_point(|t| t.time_ms < from_ms);
        let hi = self.ticks.partition_point(|t| t.time_ms < to_ms);
        &self.ticks[lo..hi]
    }
}

/// Cache key of the columns a backend currently holds; `None` forces the next resample.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ColumnsKey {
    /// Covered time range in pane-relative milliseconds.
    t_lo: f32,
    t_hi: f32,
    time_to_px: f32,
    revision: u64,
}

/// One resample result: column instances for the volume pass and the per-side visible maxima.
pub struct ColumnsUpdate {
    pub columns: Vec<ChartCross>,
    pub buy_max: f32,
    pub sell_max: f32,
}

/// Accumulates the tape into screen-space columns when the cached `key` no longer covers the
/// view; returns `None` while the cache is still valid.
///
/// Columns cover the visible window plus a `RESAMPLE_MARGIN` fraction on both sides, so panning
/// inside the margin reuses the uploaded buffer and only zooming, new data, or leaving the
/// margin rebuilds it. Every trade lands in the column its time falls into — raw sums, no
/// smoothing and no time grid, so the stepped per-tick structure stays intact. Buy columns are
/// emitted before sell columns: with alpha blending the draw order is part of the look, and
/// Moonbot draws sell over buy.
pub fn resample_if_stale(
    tape: &mut VolumeTape,
    key: &mut Option<ColumnsKey>,
    epoch_ms: f64,
    view_time0: f32,
    time_to_px: f32,
    window_px: f32,
) -> Option<ColumnsUpdate> {
    if time_to_px <= 0.0 || !time_to_px.is_finite() || window_px <= 0.0 {
        return None;
    }
    let window_ms = window_px / time_to_px;
    let margin_ms = window_ms * RESAMPLE_MARGIN;
    let need_lo = view_time0 - margin_ms * 0.5;
    let need_hi = view_time0 + window_ms + margin_ms * 0.5;
    if let Some(k) = key {
        if k.revision == tape.revision
            && k.time_to_px == time_to_px
            && k.t_lo <= need_lo
            && k.t_hi >= need_hi
        {
            return None;
        }
    }
    let step_ms = (COLUMN_STEP_PX / time_to_px).max(0.001);
    // The grid is anchored to absolute time, not to the view: `t_lo` snaps to a step multiple,
    // so every rebuild at the same zoom lands trades in the same columns. Anchored to the view
    // it drifted with the live scroll, and each incoming batch re-gridded the whole graph a
    // couple of pixels sideways — a permanent shimmer.
    let t_lo = ((view_time0 - margin_ms) / step_ms).floor() * step_ms;
    let t_hi = view_time0 + window_ms + margin_ms;
    *key = Some(ColumnsKey {
        t_lo,
        t_hi,
        time_to_px,
        revision: tape.revision,
    });

    let count = ((((t_hi - t_lo) / step_ms).ceil() as usize) + 1).min(65_536);
    let mut buys = vec![0.0f32; count];
    let mut sells = vec![0.0f32; count];
    for tick in tape.range(epoch_ms + f64::from(t_lo), epoch_ms + f64::from(t_hi)) {
        let t_rel = (tick.time_ms - epoch_ms) as f32;
        let idx = (((t_rel - t_lo) / step_ms) as usize).min(count - 1);
        if tick.sell {
            sells[idx] += tick.quote;
        } else {
            buys[idx] += tick.quote;
        }
    }

    // The normalization maxima cover ONLY the visible window, not the resample margins: the
    // scale labels must state exactly what the tallest column on screen is worth, and a whale
    // hiding just off-screen in a margin must not shrink everything the trader can see.
    let vis_lo = (((view_time0 - t_lo) / step_ms).floor() as isize).max(0) as usize;
    let vis_hi = ((((view_time0 + window_ms) - t_lo) / step_ms)
        .ceil()
        .max(0.0) as usize)
        .min(count);
    let visible = vis_lo..vis_hi;

    let mut columns = Vec::new();
    let mut buy_max = 0.0f32;
    let mut sell_max = 0.0f32;
    for (idx, &buy) in buys.iter().enumerate() {
        if buy > 0.0 {
            if visible.contains(&idx) {
                buy_max = buy_max.max(buy);
            }
            columns.push(ChartCross {
                time_rel: t_lo + step_ms * idx as f32,
                price: 0.0,
                side: 0,
                qty: buy,
            });
        }
    }
    for (idx, &sell) in sells.iter().enumerate() {
        if sell > 0.0 {
            if visible.contains(&idx) {
                sell_max = sell_max.max(sell);
            }
            columns.push(ChartCross {
                time_rel: t_lo + step_ms * idx as f32,
                price: 0.0,
                side: 1,
                qty: sell,
            });
        }
    }
    Some(ColumnsUpdate {
        columns,
        buy_max,
        sell_max,
    })
}

/// Formats a quote-currency amount the way Moonbot labels its volume scale: "174 k$", "1.2 m$".
pub fn format_quote_short(value: f32) -> String {
    if !value.is_finite() || value <= 0.0 {
        return "0 $".to_string();
    }
    if value >= 999_500.0 {
        let m = value / 1_000_000.0;
        if m >= 10.0 {
            format!("{m:.0} m$")
        } else {
            format!("{m:.1} m$")
        }
    } else if value >= 999.5 {
        let k = value / 1_000.0;
        if k >= 10.0 {
            format!("{k:.0} k$")
        } else {
            format!("{k:.1} k$")
        }
    } else {
        format!("{value:.0} $")
    }
}

#[cfg(test)]
mod tests;
