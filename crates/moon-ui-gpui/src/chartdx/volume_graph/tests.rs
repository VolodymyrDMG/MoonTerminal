use moon_core::feed::{Side, Tick};

use super::{BUCKET_MS, VolumeBuckets, format_quote_short, resample_if_stale};

fn tick(time_ms: f64, price: f32, qty: f32, side: Side) -> Tick {
    Tick {
        time_ms,
        price,
        qty,
        side,
    }
}

/// Ingests one tick per side into the same bucket and checks the quote-unit split.
#[test]
fn ingest_sums_quote_units_per_side() {
    let mut buckets = VolumeBuckets::default();
    buckets.ingest(&[
        tick(1_000_000.0, 2.0, 3.0, Side::Buy),
        tick(1_000_100.0, 2.0, 5.0, Side::Sell),
    ]);
    // Sample exactly at that bucket's center: the 3-tap average spreads the sums over three
    // buckets, so the center value is one third of the raw sums.
    let bucket = (1_000_000.0f64 / BUCKET_MS).floor();
    let center = (bucket + 0.5) * BUCKET_MS;
    let mut key = None;
    let update = resample_if_stale(&buckets, &mut key, center - 500.0, 0.0, 1.0, 1_000.0)
        .expect("first resample always runs");
    assert!((update.buy_max - 2.0).abs() < 1e-3, "{}", update.buy_max);
    assert!(
        (update.sell_max - 10.0 / 3.0).abs() < 1e-3,
        "{}",
        update.sell_max
    );
}

/// Corrupt ticks must not contribute: non-finite or non-positive quote volume is skipped.
#[test]
fn ingest_skips_corrupt_ticks() {
    let mut buckets = VolumeBuckets::default();
    buckets.ingest(&[
        tick(f64::NAN, 2.0, 3.0, Side::Buy),
        tick(1_000.0, f32::NAN, 3.0, Side::Buy),
        tick(1_000.0, 2.0, 0.0, Side::Buy),
        tick(1_000.0, -1.0, 3.0, Side::Buy),
    ]);
    assert!(buckets.is_empty());
}

/// The resample cache holds while the view stays inside the margin and the data is unchanged.
#[test]
fn resample_cache_holds_within_margin_and_invalidates_on_data() {
    let mut buckets = VolumeBuckets::default();
    buckets.ingest(&[tick(10_000.0, 1.0, 1.0, Side::Buy)]);
    let mut key = None;
    assert!(resample_if_stale(&buckets, &mut key, 0.0, 0.0, 1.0, 1_000.0).is_some());
    // Same view again: cached.
    assert!(resample_if_stale(&buckets, &mut key, 0.0, 0.0, 1.0, 1_000.0).is_none());
    // A small pan inside the margin: still cached.
    assert!(resample_if_stale(&buckets, &mut key, 0.0, 60.0, 1.0, 1_000.0).is_none());
    // New data invalidates.
    buckets.ingest(&[tick(20_000.0, 1.0, 1.0, Side::Sell)]);
    assert!(resample_if_stale(&buckets, &mut key, 0.0, 60.0, 1.0, 1_000.0).is_some());
    // A zoom change invalidates.
    assert!(resample_if_stale(&buckets, &mut key, 0.0, 60.0, 2.0, 1_000.0).is_some());
}

/// Column output: buys precede sells (draw order is part of the look), zero samples are elided,
/// and every emitted column stays within the per-side maxima the update reports.
#[test]
fn columns_are_side_ordered_and_bounded_by_maxima() {
    let mut buckets = VolumeBuckets::default();
    buckets.ingest(&[
        tick(30_000.0, 1.0, 4.0, Side::Buy),
        tick(35_000.0, 1.0, 2.0, Side::Sell),
    ]);
    let mut key = None;
    let update = resample_if_stale(&buckets, &mut key, 0.0, 20_000.0, 0.01, 300.0)
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

/// An empty bucket store resamples to an empty column set with zero maxima.
#[test]
fn empty_buckets_resample_to_no_columns() {
    let buckets = VolumeBuckets::default();
    let mut key = None;
    let update = resample_if_stale(&buckets, &mut key, 0.0, 0.0, 1.0, 500.0)
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
