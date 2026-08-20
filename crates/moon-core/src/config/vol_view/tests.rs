use super::*;

/// EVERY field survives the TOML round trip used by vol_view.toml.
#[test]
fn vol_view_roundtrip_preserves_every_field() {
    let mut file = VolViewFile::default();
    file.view.enabled = false;
    file.view.height = VOL_HEIGHT_L;
    file.view.cvd = false;
    file.view.window_secs = 300;

    let text = toml::to_string_pretty(&file).expect("serialize");
    let back: VolViewFile = toml::from_str(&text).expect("parse");
    assert_eq!(back, file);
}

/// A partial or empty file (from before a field existed) completes with defaults — most
/// importantly `enabled = true`, because the band predates the switch.
#[test]
fn vol_view_partial_toml_fills_defaults() {
    let file: VolViewFile = toml::from_str("[view]\nheight = 0\n").expect("partial parse");
    assert!(file.view.enabled);
    assert_eq!(file.view.height, VOL_HEIGHT_S);
    assert!(file.view.cvd);
    assert_eq!(file.view.window_secs, 60);

    let empty: VolViewFile = toml::from_str("").expect("empty parse");
    assert_eq!(empty.view, VolViewCfg::default());
}

/// Band geometry follows the configured height, and unknown stored bytes read as M.
#[test]
fn vol_view_band_geometry_maps_heights() {
    let mut cfg = VolViewCfg::default();
    for (h, frac, cap) in [
        (VOL_HEIGHT_S, 0.12, 160.0),
        (VOL_HEIGHT_M, 0.22, 260.0),
        (VOL_HEIGHT_L, 0.32, 380.0),
        (200u8, 0.22, 260.0),
    ] {
        cfg.height = h;
        assert_eq!(cfg.band_fraction(), frac, "height {h}");
        assert_eq!(cfg.band_max_px(), cap, "height {h}");
    }
}

/// The header window clamps to 5..=3600 and `0` reads as the default 60.
#[test]
fn vol_view_window_clamps() {
    let mut cfg = VolViewCfg::default();
    for (raw, want) in [
        (0u16, 60u32),
        (5, 5),
        (2, 5),
        (60, 60),
        (3600, 3600),
        (65000, 3600),
    ] {
        cfg.window_secs = raw;
        assert_eq!(cfg.window_secs_clamped(), want, "raw {raw}");
    }
}
