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
    let update = resample_if_stale(&mut tape, &mut key, 0.0, 5_000.0, 1.0, 30_000.0)
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
    let update = resample_if_stale(&mut tape, &mut key, 9_000.0, 0.0, 1.0, 2_000.0)
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
    let update = resample_if_stale(&mut tape, &mut key, 0.0, 5_000.0, 0.1, 3_000.0)
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
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 0.0, 1.0, 1_000.0).is_some());
    // Same view again: cached.
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 0.0, 1.0, 1_000.0).is_none());
    // A small pan inside the margin: still cached.
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 60.0, 1.0, 1_000.0).is_none());
    // New data invalidates.
    tape.ingest(&[tick(20_000.0, 1.0, 1.0, Side::Sell)]);
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 60.0, 1.0, 1_000.0).is_some());
    // A zoom change invalidates.
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 60.0, 2.0, 1_000.0).is_some());
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
    let update = resample_if_stale(&mut tape, &mut key, 0.0, 20_000.0, 0.01, 300.0)
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
    let update = resample_if_stale(&mut tape, &mut key, 0.0, 0.0, 1.0, 500.0)
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

/// A single whale no longer owns the scale: the ceiling is the 98th percentile of the non-zero
/// columns, and the whale's column simply exceeds it (the shader clips it at the band top).
#[test]
fn whale_saturates_against_percentile_ceiling() {
    let mut tape = VolumeTape::default();
    let mut batch = Vec::new();
    for i in 0..60 {
        batch.push(tick(10_000.0 + 5_000.0 * i as f64, 1.0, 10.0, Side::Buy));
    }
    batch.push(tick(400_000.0, 1.0, 1_000.0, Side::Buy));
    tape.ingest(&batch);
    let mut key = None;
    // 0.01 px/ms: trades 5 s apart sit 50 px apart, each in its own column.
    let update = resample_if_stale(&mut tape, &mut key, 0.0, 0.0, 0.01, 4_000.0)
        .expect("first resample always runs");
    assert_eq!(
        update.buy_max, 10.0,
        "p98 of sixty 10$ columns plus one whale is 10$"
    );
    let whale = update.columns.iter().map(|c| c.qty).fold(0.0f32, f32::max);
    assert_eq!(
        whale, 1_000.0,
        "the whale column itself keeps its true value"
    );
}
