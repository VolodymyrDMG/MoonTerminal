//! Half-second buy/sell volume band: the tick tape aggregated into fixed 500 ms buckets, each
//! bucket drawn as a PAIR of side-by-side columns — buys on the left, sells on the right — whose
//! heights are the per-side quote sums of that half second, normalized so the largest visible
//! bucket side touches the band top.
//!
//! The tape is the same one the combo ring consumes: every trade contributes `qty * price` at its
//! own time, retained across zooms, backfilled from the same history reads that fill the crosses.
//! Buckets anchor to ABSOLUTE time (bucket index = ⌊unix_ms / 500⌋), so a resample at any camera
//! position lands every trade in the same bucket — no drift, no shimmer.
//!
//! Only the aggregation lives here; the Metal backend draws the emitted instances as a live
//! layer, and the drag-measure overlay sums the very same tape.

// The live consumer is the Metal layer for now; on other platforms the resample chain is
// intentionally dormant until their ports land, so its items would read as dead there.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use moon_core::feed::{Side, Tick};

use super::types::ChartCross;

/// Fixed aggregation bucket in milliseconds — the user's "half-second segments".
pub const BUCKET_MS: f64 = 500.0;

/// Half-width basis carried to the shader through the instance's `price` field: each column is
/// drawn `price × time_to_px × BAR_FILL` pixels wide, so the pair fills most of its bucket at any
/// zoom without the shader knowing the bucket size.
pub const BAR_HALF_MS: f32 = 250.0;

/// Fraction of its half-bucket a column fills; the rest is the gap that keeps pairs readable.
pub const BAR_FILL: f32 = 0.78;

/// Opacity of the volume layer.
pub const GRAPH_ALPHA: f32 = 0.8;

/// Faint plate under the whole band and its hairline top edge.
pub const BAND_UNDERLAY_RGBA: [f32; 4] = [1.0, 1.0, 1.0, 0.045];
pub const BAND_EDGE_RGBA: [f32; 4] = [1.0, 1.0, 1.0, 0.10];

/// Headroom multiplier over the visible maximum when normalizing column heights: the tallest
/// bucket never touches the band ceiling, and the scale labels sit at their true heights.
pub const SCALE_HEADROOM: f32 = 1.28;

/// Default band fraction / pixel cap, the M height (see `VolViewCfg`).
pub const BAND_FRACTION: f32 = 0.22;
pub const BAND_MAX_PX: f32 = 260.0;

/// Retained tape length in trades; the oldest are pruned on ingest.
const RETAIN_TICKS: usize = 400_000;

/// Extra window fraction accumulated on each side so panning inside the margin needs no rebuild.
const RESAMPLE_MARGIN: f32 = 0.25;

/// Returns the band height in device pixels for a pane of height `pane_h` at the configured
/// band `frac`tion and pixel `cap`. Text labels and the measure overlay position against this;
/// the shader receives the same value through the view uniform's `volume_band_px`.
pub fn band_height_px(pane_h: f32, frac: f32, cap: f32) -> f32 {
    (pane_h * frac).min(cap)
}

/// One retained trade of the volume tape: absolute time, quote volume, and side.
#[derive(Clone, Copy)]
struct VolTick {
    time_ms: f64,
    quote: f32,
    sell: bool,
}

/// Retained per-pane tick tape feeding the volume band.
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
    /// the band. Batches normally arrive time-ordered; a stray unordered row only marks the
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

    /// Sum the buy and sell quote volume of trades in `[from_ms, to_ms)`.
    ///
    /// Feeds the drag-measure overlay; the window is tiny against the tape, and the slice comes
    /// from the same binary search the resampler uses.
    pub fn sum_window(&mut self, from_ms: f64, to_ms: f64) -> (f32, f32) {
        let mut buy = 0.0f32;
        let mut sell = 0.0f32;
        for t in self.range(from_ms, to_ms) {
            if t.sell {
                sell += t.quote;
            } else {
                buy += t.quote;
            }
        }
        (buy, sell)
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

/// One resample result: paired half-second columns and the per-side visible maxima.
pub struct ColumnsUpdate {
    pub columns: Vec<ChartCross>,
    pub buy_max: f32,
    pub sell_max: f32,
}

/// Accumulates the tape into half-second bucket pairs when the cached `key` no longer covers the
/// view; returns `None` while the cache is still valid.
///
/// Buckets cover the visible window plus a `RESAMPLE_MARGIN` fraction on both sides, so panning
/// inside the margin reuses the uploaded buffer and only new data, zooming, or leaving the margin
/// rebuilds it. Each bucket emits up to two instances: the buy sum centered on the bucket's first
/// quarter (`side=0`) and the sell sum on its third quarter (`side=1`), so the pair reads buys
/// left, sells right. `price` carries [`BAR_HALF_MS`], from which the shader derives the column
/// width at the current zoom.
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
    // Bucket boundaries anchor to ABSOLUTE Unix time so every rebuild lands trades in the same
    // half second; the covered range snaps outward to whole buckets.
    let abs_lo = (((epoch_ms + f64::from(view_time0 - margin_ms)) / BUCKET_MS).floor()) * BUCKET_MS;
    let abs_hi =
        (((epoch_ms + f64::from(view_time0 + window_ms + margin_ms)) / BUCKET_MS).ceil()) * BUCKET_MS;
    let t_lo = (abs_lo - epoch_ms) as f32;
    let t_hi = (abs_hi - epoch_ms) as f32;
    *key = Some(ColumnsKey {
        t_lo,
        t_hi,
        time_to_px,
        revision: tape.revision,
    });

    let count = (((abs_hi - abs_lo) / BUCKET_MS) as usize + 1).min(131_072);
    let mut buys = vec![0.0f32; count];
    let mut sells = vec![0.0f32; count];
    for tick in tape.range(abs_lo, abs_hi) {
        let idx = (((tick.time_ms - abs_lo) / BUCKET_MS) as usize).min(count - 1);
        if tick.sell {
            sells[idx] += tick.quote;
        } else {
            buys[idx] += tick.quote;
        }
    }

    // The normalization maxima cover ONLY the visible window, not the resample margins: the
    // scale labels must state exactly what the tallest visible bucket side is worth, and a whale
    // hiding just off-screen in a margin must not shrink everything the trader can see.
    let vis_lo_ms = epoch_ms + f64::from(view_time0);
    let vis_hi_ms = epoch_ms + f64::from(view_time0 + window_ms);
    let visible = |idx: usize| {
        let start = abs_lo + idx as f64 * BUCKET_MS;
        start + BUCKET_MS > vis_lo_ms && start < vis_hi_ms
    };

    let mut columns = Vec::new();
    let mut buy_max = 0.0f32;
    let mut sell_max = 0.0f32;
    for idx in 0..count {
        let start_rel = t_lo + (idx as f64 * BUCKET_MS) as f32;
        let buy = buys[idx];
        let sell = sells[idx];
        if buy > 0.0 {
            if visible(idx) {
                buy_max = buy_max.max(buy);
            }
            columns.push(ChartCross {
                time_rel: start_rel + (BUCKET_MS * 0.25) as f32,
                price: BAR_HALF_MS,
                side: 0,
                qty: buy,
            });
        }
        if sell > 0.0 {
            if visible(idx) {
                sell_max = sell_max.max(sell);
            }
            columns.push(ChartCross {
                time_rel: start_rel + (BUCKET_MS * 0.75) as f32,
                price: BAR_HALF_MS,
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
