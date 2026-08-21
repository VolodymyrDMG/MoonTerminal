use super::*;

/// `0` in KeepInChart/AddToChart is a MEANING, so garbage has to yield `None` — and with it the
/// caller's default — rather than quietly folding to zero and reading as one of those meanings.
#[test]
fn garbage_is_none_not_zero() {
    assert_eq!(field_num(&FieldValue::Double(f64::NAN)), None);
    assert_eq!(field_num(&FieldValue::Single(f32::NAN)), None);
    assert_eq!(field_num(&FieldValue::Double(f64::INFINITY)), None);
    assert_eq!(field_num(&FieldValue::Int32(-1)), None);
    assert_eq!(field_num(&FieldValue::Int64(-1)), None);
    assert_eq!(field_num(&FieldValue::String("60".into())), None);
}

/// Too large is garbage too: neither wrapping modulo 2^32, which would fake a `0`, nor
/// saturating, which would open AddToChart tab number 4294967295.
#[test]
fn oversized_is_none_neither_wraps_nor_saturates() {
    assert_eq!(field_num(&FieldValue::UInt64(1u64 << 32)), None);
    assert_eq!(field_num(&FieldValue::Int64(1i64 << 32)), None);
    assert_eq!(field_num(&FieldValue::Double(1e30)), None);
    // The upper bound itself is a value, not garbage.
    assert_eq!(
        field_num(&FieldValue::UInt64(u32::MAX as u64)),
        Some(u32::MAX)
    );
}

#[test]
fn plain_values_pass_through() {
    assert_eq!(field_num(&FieldValue::Int32(0)), Some(0));
    assert_eq!(field_num(&FieldValue::Int32(60)), Some(60));
    assert_eq!(field_num(&FieldValue::Double(60.0)), Some(60));
    assert_eq!(field_num(&FieldValue::Bool(true)), Some(1));
}

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
