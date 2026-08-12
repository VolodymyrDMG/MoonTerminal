use moonproto::{FieldValue, StrategyFields, StrategyKind, StrategySnapshot};

use super::alert_params;

/// A snapshot with the given fields and no schema, the common live-feed shape for strategies
/// whose values differ from their schema defaults (the server only transmits those).
fn snap(fields: &[(&str, FieldValue)]) -> StrategySnapshot {
    let mut f = StrategyFields::new();
    for (name, value) in fields {
        f.insert(*name, value.clone());
    }
    StrategySnapshot::new(1, 1, 0, true, StrategyKind::from_ordinal(13), "", f)
}

/// Moonbot's signal-chart switch is `SilentNoCharts` INVERTED, and unknown must read as silent:
/// dropping the inversion (or defaulting open) would open charts for every strategy that never
/// mentioned the field — exactly the yank-the-user behavior the setting exists to prevent.
#[test]
fn open_chart_inverts_silent_no_charts_and_unknown_stays_silent() {
    let silent = snap(&[("SilentNoCharts", FieldValue::Bool(true))]);
    assert!(!alert_params(&silent, None).open_chart);

    let loud = snap(&[("SilentNoCharts", FieldValue::Bool(false))]);
    assert!(alert_params(&loud, None).open_chart);

    // No field and no schema: stay silent rather than guess.
    assert!(!alert_params(&snap(&[]), None).open_chart);

    // The neighbors the detect row carries alongside it keep their meanings.
    let charted = snap(&[
        ("AddToChart", FieldValue::Int32(2)),
        ("KeepInChart", FieldValue::Int32(90)),
    ]);
    let p = alert_params(&charted, None);
    assert_eq!(p.add_to_chart, 2);
    assert_eq!(p.keep_in_chart_secs, 90);
}
