//! Moonbot-style time-volume graph: 5-second buy/sell buckets in quote units, resampled into
//! dense screen-space columns for the existing per-instance volume pass.
//!
//! The old volume pass drew one hairline bar per trade cross, which reads as noise on liquid
//! markets. Moonbot instead aggregates trades into fixed time buckets, splits them by side,
//! smooths the series, and draws it as two overlapping translucent area graphs at the bottom of
//! the chart with a quote-currency scale. This module owns the platform-free part of that:
//! incremental bucket sums fed from the same tick batches the combo ring consumes, and a
//! view-dependent resampler that emits one `ChartCross`-encoded column per couple of pixels so
//! the unmodified column shader renders a solid filled graph.
//!
//! Only the aggregation lives here; backends draw the emitted columns as a live layer.

// The live consumer is the Metal layer for now; on other platforms the resample chain is
// intentionally dormant until their ports land, so its items would read as dead there.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::collections::BTreeMap;

use moon_core::feed::{Side, Tick};

use super::types::ChartCross;

/// Aggregation bucket width in absolute milliseconds, matching Moonbot's default "Time, sec: 5".
pub const BUCKET_MS: f64 = 5_000.0;

/// Horizontal distance between emitted columns in pane pixels.
///
/// The volume shader draws each column 2.75 px wide, so a 2 px step keeps neighbouring columns
/// overlapping into a solid area at every zoom level.
pub const COLUMN_STEP_PX: f32 = 2.0;

/// Opacity of the volume graph layer, denser than the old per-trade bars to read as an area.
pub const GRAPH_ALPHA: f32 = 0.55;

/// Fraction of the pane height the graph band may occupy, mirrored by the volume shader.
pub const BAND_FRACTION: f32 = 0.22;

/// Hard cap of the band height in pixels, mirrored by the volume shader.
pub const BAND_MAX_PX: f32 = 260.0;

/// Retention horizon in buckets; older sums are pruned on ingest (about 27 hours at 5 s).
const RETAIN_BUCKETS: i64 = 20_000;

/// Extra window fraction resampled on each side so panning inside the margin needs no rebuild.
const RESAMPLE_MARGIN: f32 = 0.25;

/// Returns the graph band height in device pixels for a pane of height `pane_h`.
///
/// Text labels position against this, so it must stay equal to the shader's band expression.
pub fn band_height_px(pane_h: f32) -> f32 {
    (pane_h * BAND_FRACTION).min(BAND_MAX_PX)
}

/// Incrementally maintained per-bucket quote-volume sums for one pane's market.
///
/// Keys are absolute bucket indices (`floor(time_ms / BUCKET_MS)`), so the sums are independent
/// of the pane's render epoch and survive zooming untouched. Values are `(buy, sell)` sums in
/// quote units (`qty * price`). `revision` increments on every content change and keys the
/// resample cache.
#[derive(Default)]
pub struct VolumeBuckets {
    sums: BTreeMap<i64, (f32, f32)>,
    pub revision: u64,
}

impl VolumeBuckets {
    /// Drops all sums, for market switches and full history resets.
    pub fn reset(&mut self) {
        if !self.sums.is_empty() {
            self.sums.clear();
        }
        self.revision = self.revision.wrapping_add(1);
    }

    /// Adds a tick batch to the bucket sums and prunes buckets beyond the retention horizon.
    ///
    /// Quantities are in base units, so each trade contributes `qty * price` quote units to its
    /// side. Non-finite and non-positive contributions are skipped: one corrupt tick must not
    /// poison a whole bucket.
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
            let idx = (tick.time_ms / BUCKET_MS).floor() as i64;
            let entry = self.sums.entry(idx).or_insert((0.0, 0.0));
            match tick.side {
                Side::Buy => entry.0 += quote,
                Side::Sell => entry.1 += quote,
            }
            changed = true;
        }
        if changed {
            if let Some((&newest, _)) = self.sums.last_key_value() {
                let cutoff = newest - RETAIN_BUCKETS;
                // BTreeMap::retain walks everything; splitting off the live tail touches only
                // the pruned head, which is empty on the hot path.
                if self
                    .sums
                    .first_key_value()
                    .is_some_and(|(&oldest, _)| oldest < cutoff)
                {
                    self.sums = self.sums.split_off(&cutoff);
                }
            }
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Returns the smoothed per-side value at bucket index `idx` using a 3-tap moving average.
    fn smoothed(&self, idx: i64) -> (f32, f32) {
        let mut buy = 0.0;
        let mut sell = 0.0;
        for i in idx - 1..=idx + 1 {
            if let Some(&(b, s)) = self.sums.get(&i) {
                buy += b;
                sell += s;
            }
        }
        (buy / 3.0, sell / 3.0)
    }

    /// Linearly interpolates the smoothed series at absolute time `time_ms`.
    ///
    /// Sample points sit at bucket centers, which turns the 5-second staircase into the
    /// continuous curve Moonbot's "Smooth graph" mode draws.
    fn sample(&self, time_ms: f64) -> (f32, f32) {
        let pos = time_ms / BUCKET_MS - 0.5;
        let left = pos.floor();
        let frac = (pos - left) as f32;
        let (b0, s0) = self.smoothed(left as i64);
        let (b1, s1) = self.smoothed(left as i64 + 1);
        (b0 + (b1 - b0) * frac, s0 + (s1 - s0) * frac)
    }

    /// True when no bucket holds any volume.
    pub fn is_empty(&self) -> bool {
        self.sums.is_empty()
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

/// Resamples `buckets` into screen-space columns when the cached `key` no longer covers the
/// view; returns `None` while the cache is still valid.
///
/// Columns cover the visible window plus a `RESAMPLE_MARGIN` fraction on both sides, so panning
/// inside the margin reuses the uploaded buffer and only zooming, new data, or leaving the
/// margin rebuilds it. Buy columns are emitted before sell columns: with alpha blending the
/// draw order is part of the look, and Moonbot draws sell over buy.
pub fn resample_if_stale(
    buckets: &VolumeBuckets,
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
        if k.revision == buckets.revision
            && k.time_to_px == time_to_px
            && k.t_lo <= need_lo
            && k.t_hi >= need_hi
        {
            return None;
        }
    }
    let t_lo = view_time0 - margin_ms;
    let t_hi = view_time0 + window_ms + margin_ms;
    *key = Some(ColumnsKey {
        t_lo,
        t_hi,
        time_to_px,
        revision: buckets.revision,
    });

    let mut columns = Vec::new();
    let mut buy_max = 0.0f32;
    let mut sell_max = 0.0f32;
    if !buckets.is_empty() {
        let step_ms = (COLUMN_STEP_PX / time_to_px).max(1.0);
        let count = (((t_hi - t_lo) / step_ms).ceil() as usize).min(65_536);
        let mut samples = Vec::with_capacity(count);
        for i in 0..count {
            let t_rel = t_lo + step_ms * i as f32;
            let (buy, sell) = buckets.sample(epoch_ms + f64::from(t_rel));
            buy_max = buy_max.max(buy);
            sell_max = sell_max.max(sell);
            samples.push((t_rel, buy, sell));
        }
        columns.reserve(samples.len() * 2);
        for &(t_rel, buy, _) in &samples {
            if buy > 0.0 {
                columns.push(ChartCross {
                    time_rel: t_rel,
                    price: 0.0,
                    side: 0,
                    qty: buy,
                });
            }
        }
        for &(t_rel, _, sell) in &samples {
            if sell > 0.0 {
                columns.push(ChartCross {
                    time_rel: t_rel,
                    price: 0.0,
                    side: 1,
                    qty: sell,
                });
            }
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
        format!("{:.0} k$", value / 1_000.0)
    } else {
        format!("{value:.0} $")
    }
}

#[cfg(test)]
mod tests;
