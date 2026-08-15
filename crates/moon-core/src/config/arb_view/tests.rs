use super::*;

/// The overlay must stay dark until asked and readable once asked: defaults are off/lines/
/// numbers/percent, an unknown platform is enabled with a deterministic palette color, and the
/// whole file round-trips through TOML (the Copy/share path of the future).
#[test]
fn arb_view_defaults_platform_fallback_and_roundtrip() {
    let cfg = ArbViewCfg::default();
    assert!(
        !cfg.enabled,
        "an empty config must not grow chart legends unasked"
    );
    assert!(cfg.lines && cfg.numbers && cfg.percent);
    assert!(!cfg.prices && !cfg.right);

    // Unknown platform: enabled, palette color, and the SAME color on every call/machine.
    let a = cfg.platform("BybitF");
    let b = cfg.platform("BybitF");
    assert!(a.on);
    assert_eq!(a.color, b.color);
    assert_ne!(
        a.color,
        cfg.platform("GateS").color,
        "distinct names should spread hues"
    );

    // Explicit override wins and survives serialization.
    let mut file = ArbViewFile::default();
    file.view.enabled = true;
    file.view.right = true;
    file.view.set_platform(
        "GateS",
        ArbPlatformView {
            on: false,
            color: [1, 2, 3],
        },
    );
    let text = toml::to_string_pretty(&file).expect("serialize");
    let back: ArbViewFile = toml::from_str(&text).expect("parse");
    assert_eq!(back, file);
    assert!(!back.view.platform("GateS").on);
    assert_eq!(back.view.platform("GateS").color, [1, 2, 3]);

    // Old/foreign file without new fields completes from defaults.
    let old: ArbViewFile = toml::from_str("").expect("empty parse");
    assert_eq!(old, ArbViewFile::default());
}
