//! Regression tests for detection presentation scoping.

use super::detection_core_visible;

/// `detects/mod.rs:ingest` filtering by the effective Auto core would advance every cursor while
/// dropping hidden cards, so returning to Overview could neither reveal nor replay those detects.
#[test]
fn presentation_scope_keeps_hidden_detection_cards_retained() {
    let retained = vec![11, 22, 11];

    let selected: Vec<u64> = retained
        .iter()
        .copied()
        .filter(|core| detection_core_visible(*core, &[22]))
        .collect();
    assert_eq!(selected, vec![22]);
    assert_eq!(retained, vec![11, 22, 11]);
    assert!(
        retained
            .iter()
            .all(|core| detection_core_visible(*core, &[11, 22]))
    );

    let src = include_str!("mod.rs");
    let ingest = src
        .split("fn ingest(")
        .nth(1)
        .and_then(|tail| tail.split("\n    }").next())
        .expect("Detects ingest must exist");
    assert!(ingest.contains(".filter(|s| s.group == self.group)"));
    assert!(!ingest.contains("effective_workspace_scope"));

    let render = src
        .split("impl Render for DetectsPanel")
        .nth(1)
        .expect("Detects render must exist");
    assert!(render.contains("effective_workspace_scope"));
    assert!(render.contains("detection_core_visible(item.core, &visible_cores)"));
}

/// Detect cards must validate Main/Compare authority before removing their retained card.
///
/// Mutation: move either `retain` call before its authorized request. A stale card click would
/// disappear and navigate to a core hidden by the current rail selection.
#[test]
fn stale_detect_navigation_is_rejected_before_card_removal() {
    let source = include_str!("mod.rs");
    for (method, authority) in [
        ("fn open(&mut self", "open_on_main_if_authorized"),
        ("fn open_compare(&mut self", "open_compare_if_authorized"),
    ] {
        let body = source
            .split(method)
            .nth(1)
            .expect("Detect navigation method must exist");
        let guard = body.find(authority).expect("workspace guard must exist");
        let removal = body.find("self.items").expect("card removal must remain");
        assert!(
            guard < removal,
            "{method} removes a stale card before authority"
        );
    }
}

/// The hover popup's 30-second aggregates must split turnover by side, take the LAST trade as the
/// detection-moment price, and measure the window delta first-to-last — not high-to-low, which
/// would overstate every burst.
#[test]
fn hover_tick_stats_split_sides_and_measure_first_to_last() {
    use moon_core::market::DetectTick;

    use super::hover::{TickStats, price_range, tick_stats};

    let ticks = [
        DetectTick {
            t_rel_ms: -20_000.0,
            price: 100.0,
            quote: 500.0,
            sell: false,
        },
        DetectTick {
            t_rel_ms: -10_000.0,
            price: 108.0,
            quote: 300.0,
            sell: true,
        },
        DetectTick {
            t_rel_ms: -1_000.0,
            price: 104.0,
            quote: 200.0,
            sell: false,
        },
    ];
    let stats = tick_stats(&ticks);
    assert_eq!(stats.trades, 3);
    assert_eq!(stats.buy_quote, 700.0);
    assert_eq!(stats.sell_quote, 300.0);
    assert_eq!(stats.last_price, Some(104.0));
    // First 100 → last 104 is +4%, even though the window high was 108.
    let d = stats.win_pct.expect("two or more trades give a delta");
    assert!((d - 4.0).abs() < 1e-4, "first-to-last delta, got {d}");
    // The chart corner labels use the true extremes.
    assert_eq!(price_range(&ticks), Some((108.0, 100.0)));

    // No trades: everything empty rather than zeros pretending to be measurements.
    assert_eq!(tick_stats(&[]), TickStats::default());
    assert_eq!(price_range(&[]), None);

    // One trade prices the moment but cannot measure a change.
    let one = tick_stats(&ticks[..1]);
    assert_eq!(one.last_price, Some(100.0));
    assert_eq!(one.win_pct, None);
}

/// The gear popup's tick window trims the frozen rows to a SUFFIX: an exact `-window` boundary
/// stays in, older rows drop out, and the window delta measures the trimmed slice — so 5/15/30
/// answer "what happened in the last N seconds", not "since the snapshot began".
///
/// Mutation: filter with `<=` (boundary trade dropped), measure the full rows (window ignored),
/// or count one-trade windows as a 0% move instead of "—".
#[test]
fn tick_window_trims_to_suffix_and_scopes_the_delta() {
    use moon_core::market::DetectTick;

    use super::cards::{window_delta, window_slice};

    let t = |t_rel_ms: f32, price: f32| DetectTick {
        t_rel_ms,
        price,
        quote: 1.0,
        sell: false,
    };
    // 25 s of quiet decline, then a burst in the last 4 s: 100 → 90 overall, 96 → 90 in-window.
    let ticks = [
        t(-25_000.0, 100.0),
        t(-15_000.0, 96.0), // exactly on the 15 s boundary — kept
        t(-4_000.0, 93.0),
        t(-500.0, 90.0),
    ];

    assert_eq!(window_slice(&ticks, 30_000.0).len(), 4);
    assert_eq!(window_slice(&ticks, 15_000.0).len(), 3);
    assert_eq!(window_slice(&ticks, 5_000.0).len(), 2);

    let d30 = window_delta(&ticks, 30_000.0).expect("full window");
    assert!((d30 - -10.0).abs() < 1e-4, "100→90 is -10%, got {d30}");
    let d15 = window_delta(&ticks, 15_000.0).expect("15 s window");
    assert!((d15 - -6.25).abs() < 1e-4, "96→90 is -6.25%, got {d15}");

    // A window holding one trade (or none) has no first-to-last change to print.
    assert_eq!(window_delta(&ticks[..1], 30_000.0), None);
    assert_eq!(window_delta(&ticks, 0.25), None);
    assert_eq!(window_delta(&[], 30_000.0), None);
}
