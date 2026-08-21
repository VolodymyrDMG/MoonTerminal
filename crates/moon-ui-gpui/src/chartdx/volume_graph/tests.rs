use moon_core::feed::{Side, Tick};

use super::{BAR_HALF_MS, BUCKET_MS, VolumeTape, format_quote_short, resample_if_stale};

fn tick(time_ms: f64, price: f32, qty: f32, side: Side) -> Tick {
    Tick {
        time_ms,
        price,
        qty,
        side,
    }
}

/// Trades of one half second land in ONE bucket as per-side sums: buys and sells become the
/// bucket's paired columns (buy on the first quarter, sell on the third), and trades half a
/// second apart never share a bucket.
#[test]
fn half_second_buckets_sum_each_side_into_a_pair() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[
        tick(10_050.0, 2.0, 3.0, Side::Buy),  // 6$   ┐ same 10.0–10.5s bucket
        tick(10_400.0, 2.0, 1.0, Side::Buy),  // 2$   ┘
        tick(10_450.0, 2.0, 5.0, Side::Sell), // 10$  same bucket, sell side
        tick(10_600.0, 2.0, 4.0, Side::Sell), // 8$   NEXT bucket
    ]);
    let mut key = None;
    let update = resample_if_stale(&mut tape, &mut key, 0.0, 9_000.0, 0.1, 400.0)
        .expect("first resample always runs");
    let buys: Vec<_> = update.columns.iter().filter(|c| c.side == 0).collect();
    let sells: Vec<_> = update.columns.iter().filter(|c| c.side == 1).collect();
    assert_eq!(buys.len(), 1, "two buys of one half second merge");
    assert_eq!(sells.len(), 2, "sells half a second apart stay separate");
    assert_eq!(buys[0].qty, 8.0);
    assert_eq!(buys[0].time_rel, 10_000.0 + (BUCKET_MS * 0.25) as f32);
    assert_eq!(sells[0].qty, 10.0);
    assert_eq!(sells[0].time_rel, 10_000.0 + (BUCKET_MS * 0.75) as f32);
    assert_eq!(sells[1].qty, 8.0);
    assert_eq!(sells[1].time_rel, 10_500.0 + (BUCKET_MS * 0.75) as f32);
    // Every instance carries the shader's width basis.
    assert!(update.columns.iter().all(|c| c.price == BAR_HALF_MS));
    assert_eq!(update.buy_max, 8.0);
    assert_eq!(update.sell_max, 10.0);
}

/// Buckets anchor to ABSOLUTE time: two resamples with different cameras and epochs put the same
/// trade in the same half second, so panning cannot re-grid the band.
#[test]
fn buckets_anchor_to_absolute_time_across_epochs_and_cameras() {
    let trades = [
        tick(100_260.0, 1.0, 2.0, Side::Buy),
        tick(100_490.0, 1.0, 3.0, Side::Buy),
    ];
    let run = |epoch: f64, view0: f32| {
        let mut tape = VolumeTape::default();
        tape.ingest(&trades);
        let mut key = None;
        resample_if_stale(&mut tape, &mut key, epoch, view0, 0.1, 300.0)
            .unwrap()
            .columns
            .into_iter()
            .map(|c| (epoch + f64::from(c.time_rel), c.qty as i64))
            .collect::<Vec<_>>()
    };
    assert_eq!(run(0.0, 99_000.0), run(60_000.0, 39_000.0));
}

/// The normalization maxima cover ONLY the visible window: a monster bucket in the prefetch
/// margin must not shrink what the trader can actually see.
#[test]
fn maxima_ignore_margin_buckets() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[
        tick(5_100.0, 1.0, 4.0, Side::Buy),    // visible
        tick(18_100.0, 1.0, 900.0, Side::Buy), // right prefetch margin, off-screen
    ]);
    let mut key = None;
    // Window 0..16s at 0.1 px/ms (1600 px); the margin extends to 20s, so the 18.1s whale is
    // resampled (drawn when the user pans) but must not set the visible scale.
    let update = resample_if_stale(&mut tape, &mut key, 0.0, 0.0, 0.1, 1_600.0)
        .expect("first resample always runs");
    assert_eq!(update.buy_max, 4.0, "margin bucket must not set the scale");
    assert!(update.columns.iter().any(|c| c.qty == 900.0), "yet it is still drawn");
}

/// `sum_window` answers the measure overlay: per-side quote sums over a half-open range.
#[test]
fn sum_window_splits_sides_over_the_requested_range() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[
        tick(1_000.0, 2.0, 3.0, Side::Buy),    // 6$
        tick(2_000.0, 2.0, 5.0, Side::Sell),   // 10$
        tick(10_000.0, 2.0, 100.0, Side::Buy), // outside
    ]);
    assert_eq!(tape.sum_window(0.0, 3_000.0), (6.0, 10.0));
    assert_eq!(tape.sum_window(2_000.0, 3_000.0), (0.0, 10.0), "boundary trade stays in");
    assert_eq!(tape.sum_window(0.0, f64::INFINITY), (206.0, 10.0));
}

/// Corrupt ticks must not contribute: non-finite or non-positive quote volume is skipped.
#[test]
fn ingest_skips_corrupt_ticks() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[
        tick(f64::NAN, 2.0, 3.0, Side::Buy),
        tick(1_000.0, f32::NAN, 3.0, Side::Buy),
        tick(1_000.0, 2.0, 0.0, Side::Buy),
        tick(1_000.0, -2.0, 3.0, Side::Buy),
    ]);
    assert!(tape.is_empty());
}

/// An unsorted batch still lands every trade in its own half second after the lazy sort.
#[test]
fn unsorted_batches_sort_before_aggregation() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[
        tick(2_100.0, 1.0, 2.0, Side::Buy),
        tick(1_100.0, 1.0, 3.0, Side::Buy), // out of order
    ]);
    let mut key = None;
    let update = resample_if_stale(&mut tape, &mut key, 0.0, 0.0, 0.1, 400.0)
        .expect("first resample always runs");
    let buys: Vec<_> = update.columns.iter().filter(|c| c.side == 0).collect();
    assert_eq!(buys.len(), 2);
    assert_eq!(buys[0].qty, 3.0);
    assert_eq!(buys[1].qty, 2.0);
}

/// The resample cache: an unchanged tape and coverage answers `None`; new data resamples.
#[test]
fn resample_caches_until_data_or_coverage_changes() {
    let mut tape = VolumeTape::default();
    tape.ingest(&[tick(500.0, 1.0, 1.0, Side::Buy)]);
    let mut key = None;
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 0.0, 1.0, 1_000.0).is_some());
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 0.0, 1.0, 1_000.0).is_none());
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 60.0, 1.0, 1_000.0).is_none());
    tape.ingest(&[tick(600.0, 1.0, 1.0, Side::Buy)]);
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 60.0, 1.0, 1_000.0).is_some());
    assert!(resample_if_stale(&mut tape, &mut key, 0.0, 60.0, 2.0, 1_000.0).is_some());
}

/// Moonbot-style short quote formatting for the scale labels.
#[test]
fn quote_amounts_format_like_moonbot() {
    assert_eq!(format_quote_short(0.0), "0 $");
    assert_eq!(format_quote_short(742.3), "742 $");
    assert_eq!(format_quote_short(1_240.0), "1.2 k$");
    assert_eq!(format_quote_short(174_000.0), "174 k$");
    assert_eq!(format_quote_short(1_200_000.0), "1.2 m$");
    assert_eq!(format_quote_short(24_000_000.0), "24 m$");
}
