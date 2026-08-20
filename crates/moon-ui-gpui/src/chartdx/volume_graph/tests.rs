use moon_core::feed::{Side, Tick};

use super::{VolumeTape, format_quote_short, resample_if_stale};

fn tick(time_ms: f64, price: f32, qty: f32, side: Side) -> Tick {
    Tick {
        time_ms,
        price,
        qty,
        side,
    }
}

/// Two trades far enough apart land in their own columns with raw per-side quote sums.
#[test]
fn trades_accumulate_raw_quote_sums_per_column() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[
        tick(10_000.0, 2.0, 3.0, Side::Buy),
        tick(10_001.0, 2.0, 1.0, Side::Buy),
        tick(30_000.0, 2.0, 5.0, Side::Sell),
    ]);
    let mut key = None;
    // 1 px per ms: the two buys (1 ms apart) share a 2 ms column, the sell is far away.
    let update = resample_if_stale(&mut tape, &mut key, 0.0, 5_000.0, 1.0, 30_000.0, false)
        .expect("first resample always runs");
    assert_eq!(update.buy_max, 8.0, "2*3 + 2*1 in one column");
    assert_eq!(update.sell_max, 10.0);
    let buys: Vec<_> = update.columns.iter().filter(|c| c.side == 0).collect();
    let sells: Vec<_> = update.columns.iter().filter(|c| c.side == 1).collect();
    assert_eq!(buys.len(), 1);
    assert_eq!(sells.len(), 1);
    assert_eq!(buys[0].qty, 8.0);
    assert_eq!(sells[0].qty, 10.0);
}

/// Zoomed in far enough, every trade is its own column: no time grid glues them together.
#[test]
fn zoomed_in_trades_stay_separate() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[
        tick(10_000.0, 1.0, 1.0, Side::Buy),
        tick(10_050.0, 1.0, 2.0, Side::Buy),
    ]);
    let mut key = None;
    // 1 px per ms: 50 ms apart is 25 columns apart.
    let update = resample_if_stale(&mut tape, &mut key, 9_000.0, 0.0, 1.0, 2_000.0, false)
        .expect("first resample always runs");
    let buys: Vec<_> = update.columns.iter().filter(|c| c.side == 0).collect();
    assert_eq!(buys.len(), 2);
    assert_eq!(update.buy_max, 2.0, "separate trades never sum");
}

/// Corrupt ticks must not contribute: non-finite or non-positive quote volume is skipped.
#[test]
fn ingest_skips_corrupt_ticks() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[
        tick(f64::NAN, 2.0, 3.0, Side::Buy),
        tick(1_000.0, f32::NAN, 3.0, Side::Buy),
        tick(1_000.0, 2.0, 0.0, Side::Buy),
        tick(1_000.0, -1.0, 3.0, Side::Buy),
    ]);
    assert!(tape.is_empty());
}

/// An out-of-order batch still accumulates correctly thanks to the lazy sort.
#[test]
fn out_of_order_batches_accumulate_after_lazy_sort() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[tick(20_000.0, 1.0, 1.0, Side::Buy)]);
    tape.ingest(&[tick(10_000.0, 1.0, 2.0, Side::Buy)]);
    let mut key = None;
    let update = resample_if_stale(&mut tape, &mut key, 0.0, 5_000.0, 0.1, 3_000.0, false)
        .expect("first resample always runs");
    let total: f32 = update
        .columns
        .iter()
        .filter(|c| c.side == 0)
        .map(|c| c.qty)
        .sum();
    assert_eq!(total, 3.0, "both trades inside the window must count");
}

/// The resample cache holds while the view stays inside the margin and the data is unchanged.
#[test]
fn resample_cache_holds_within_margin_and_invalidates_on_data() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[tick(10_000.0, 1.0, 1.0, Side::Buy)]);
    let mut key = None;
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 0.0, 1.0, 1_000.0, false).is_some());
    // Same view again: cached.
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 0.0, 1.0, 1_000.0, false).is_none());
    // A small pan inside the margin: still cached.
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 60.0, 1.0, 1_000.0, false).is_none());
    // New data invalidates.
    tape.ingest(&[tick(20_000.0, 1.0, 1.0, Side::Sell)]);
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 60.0, 1.0, 1_000.0, false).is_some());
    // A zoom change invalidates.
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 60.0, 2.0, 1_000.0, false).is_some());
}

/// Column output keeps buys before sells (draw order is part of the look) and every column
/// stays within the per-side maxima the update reports.
#[test]
fn columns_are_side_ordered_and_bounded_by_maxima() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[
        tick(30_000.0, 1.0, 4.0, Side::Buy),
        tick(35_000.0, 1.0, 2.0, Side::Sell),
        tick(36_000.0, 1.0, 1.0, Side::Buy),
    ]);
    let mut key = None;
    let update = resample_if_stale(&mut tape, &mut key, 0.0, 20_000.0, 0.01, 300.0, false)
        .expect("first resample always runs");
    assert!(!update.columns.is_empty());
    let first_sell = update
        .columns
        .iter()
        .position(|c| c.side == 1)
        .unwrap_or(update.columns.len());
    assert!(
        update.columns[first_sell..].iter().all(|c| c.side == 1),
        "buy columns must all precede sell columns"
    );
    for column in &update.columns {
        assert!(column.qty > 0.0);
        let max = if column.side == 0 {
            update.buy_max
        } else {
            update.sell_max
        };
        assert!(column.qty <= max + 1e-6);
    }
}

/// An empty tape resamples to an empty column set with zero maxima.
#[test]
fn empty_tape_resamples_to_no_columns() {
    let mut tape = VolumeTape::default();
    let mut key = None;
    let update = resample_if_stale(&mut tape, &mut key, 0.0, 0.0, 1.0, 500.0, false)
        .expect("first resample always runs");
    assert!(update.columns.is_empty());
    assert_eq!(update.buy_max, 0.0);
    assert_eq!(update.sell_max, 0.0);
}

/// The Moonbot-style scale label formatting across magnitude ranges.
#[test]
fn quote_labels_match_moonbot_style() {
    assert_eq!(format_quote_short(512.0), "512 $");
    assert_eq!(format_quote_short(87_000.0), "87 k$");
    assert_eq!(format_quote_short(174_400.0), "174 k$");
    assert_eq!(format_quote_short(1_240_000.0), "1.2 m$");
    assert_eq!(format_quote_short(12_400_000.0), "12 m$");
    assert_eq!(format_quote_short(0.0), "0 $");
    assert_eq!(format_quote_short(f32::NAN), "0 $");
}

/// The scale is the honest maximum of the VISIBLE window: an off-screen whale sitting in the
/// resample margin is still emitted as a column but must not shrink what the trader sees.
#[test]
fn scale_uses_only_the_visible_window() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[
        tick(100_000.0, 1.0, 5.0, Side::Buy),
        // Inside the covered margin, beyond the visible right edge at 145 000.
        tick(150_000.0, 1.0, 500.0, Side::Buy),
    ]);
    let mut key = None;
    // 0.01 px/ms, 500 px window: visible range [95 000, 145 000], margins reach ~157 500.
    let update = resample_if_stale(&mut tape, &mut key, 0.0, 95_000.0, 0.01, 500.0, false)
        .expect("first resample always runs");
    assert_eq!(update.buy_max, 5.0, "the label states the visible maximum");
    let tallest = update.columns.iter().map(|c| c.qty).fold(0.0f32, f32::max);
    assert_eq!(tallest, 500.0, "the margin column itself is still emitted");
}

/// The column grid is anchored to absolute time: rebuilds at the same zoom land the same trade
/// in the same column no matter where the view is — otherwise every live-scroll rebuild
/// re-gridded the graph a couple of pixels sideways, a permanent shimmer.
#[test]
fn column_grid_is_anchored_across_rebuilds() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[tick(100_000.0, 1.0, 1.0, Side::Buy)]);
    let mut key_a = None;
    let a = resample_if_stale(&mut tape, &mut key_a, 0.0, 50_000.0, 0.01, 1_000.0, false)
        .expect("first resample always runs");
    let mut key_b = None;
    // A different, non-step-aligned view forces a fresh grid at the same zoom.
    let b = resample_if_stale(&mut tape, &mut key_b, 0.0, 57_777.0, 0.01, 1_000.0, false)
        .expect("first resample always runs");
    let ta = a.columns.first().expect("trade emitted").time_rel;
    let tb = b.columns.first().expect("trade emitted").time_rel;
    assert_eq!(ta, tb, "same zoom, same trade, same column");
}

/// Sub-10k amounts keep one decimal, as the Moonbot bracket labels do ("5.3 k$").
#[test]
fn small_k_labels_keep_a_decimal() {
    assert_eq!(format_quote_short(5_300.0), "5.3 k$");
    assert_eq!(format_quote_short(9_940.0), "9.9 k$");
}

/// The CVD flag emits one `side=2` marker per column up to the LAST trade, forward-filled
/// through empty columns and normalized so the range's low sits near 0 and its high near 1.
#[test]
fn cvd_steps_are_normalized_and_stop_at_the_last_trade() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[
        tick(1_000.0, 1.0, 10.0, Side::Buy),  // cum +10 (high)
        tick(3_000.0, 1.0, 30.0, Side::Sell), // cum -20 (low)
        tick(5_000.0, 1.0, 10.0, Side::Buy),  // cum -10
    ]);
    let mut key = None;
    // 0.01 px/ms → 200 ms columns over a 0..10s window.
    let update = resample_if_stale(&mut tape, &mut key, 0.0, 0.0, 0.01, 100.0, true)
        .expect("first resample always runs");
    let cvd: Vec<_> = update.columns.iter().filter(|c| c.side == 2).collect();
    assert!(!cvd.is_empty());
    // The line must not extend past the last trade's column into the future margin.
    let last_t = cvd.iter().map(|c| c.time_rel).fold(f32::MIN, f32::max);
    assert!(
        (5_000.0..6_000.0).contains(&last_t),
        "line ends at the last trade, got {last_t}"
    );
    // Normalized extremes: +10 → ~0.96 (top inset), −20 → ~0.04; the fill between trades holds.
    let at = |t: f32| {
        cvd.iter()
            .filter(|c| c.time_rel <= t + 1.0)
            .max_by(|a, b| a.time_rel.total_cmp(&b.time_rel))
            .expect("column at time")
            .qty
    };
    assert!((at(1_100.0) - 0.96).abs() < 0.02, "high {}", at(1_100.0));
    assert!((at(3_100.0) - 0.04).abs() < 0.02, "low {}", at(3_100.0));
    assert!((at(2_500.0) - 0.96).abs() < 0.02, "forward fill holds the last level");
    let mid = at(5_100.0);
    assert!((mid - 0.3466).abs() < 0.02, "final −10 sits a third up, got {mid}");
    // Toggling CVD alone must invalidate the cache even though coverage still fits.
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 0.0, 0.01, 100.0, false).is_some());
    // And the CVD-off result carries no side=2 instances.
    let mut key2 = None;
    let off = resample_if_stale(&mut tape, &mut key2, 0.0, 0.0, 0.01, 100.0, false).unwrap();
    assert!(off.columns.iter().all(|c| c.side != 2));
}

/// `sum_window` answers the header's Bv/Sv question: per-side quote sums over a half-open range.
#[test]
fn sum_window_splits_sides_over_the_requested_range() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[
        tick(1_000.0, 2.0, 3.0, Side::Buy),   // 6$
        tick(2_000.0, 2.0, 5.0, Side::Sell),  // 10$
        tick(10_000.0, 2.0, 100.0, Side::Buy), // outside
    ]);
    assert_eq!(tape.sum_window(0.0, 3_000.0), (6.0, 10.0));
    assert_eq!(tape.sum_window(2_000.0, 3_000.0), (0.0, 10.0), "boundary trade stays in");
    assert_eq!(tape.sum_window(0.0, f64::INFINITY), (206.0, 10.0));
}

/// An out-of-order batch rebuilds the running cumulative in time order: the CVD of a sorted and
/// an unsorted ingest of the same trades must match.
#[test]
fn unsorted_ingest_recomputes_the_cumulative() {
    let trades = [
        tick(1_000.0, 1.0, 4.0, Side::Buy),
        tick(2_000.0, 1.0, 1.0, Side::Sell),
        tick(3_000.0, 1.0, 2.0, Side::Buy),
    ];
    let mut sorted = VolumeTape::default();
    sorted.ingest(&trades);
    let mut shuffled = VolumeTape::default();
    shuffled.ingest(&[trades[2], trades[0], trades[1]]);
    let run = |tape: &mut VolumeTape| {
        let mut key = None;
        resample_if_stale(tape, &mut key, 0.0, 0.0, 0.01, 100.0, true)
            .unwrap()
            .columns
            .into_iter()
            .filter(|c| c.side == 2)
            .map(|c| (c.time_rel as i64, (c.qty * 1000.0) as i32))
            .collect::<Vec<_>>()
    };
    assert_eq!(run(&mut sorted), run(&mut shuffled));
}

/// The Metal backend normalizes BOTH sides by ONE shared maximum — the scale labels print a
/// single number, so per-side normalization would draw the quiet side taller than labeled.
/// Anchored on the source because the backend type only compiles on macOS.
#[test]
fn volume_columns_normalize_both_sides_by_the_shared_maximum() {
    let source = include_str!("../metal_backend.rs");
    let body = source
        .split("pub fn set_volume_columns(")
        .nth(1)
        .and_then(|tail| tail.split("pub fn set_vol_band(").next())
        .expect("set_volume_columns must precede set_vol_band");
    assert!(body.contains("let shared = buy_max.max(sell_max).max(1e-6);"));
    assert!(body.contains("self.volume_columns_scale = (shared, shared);"));
}
