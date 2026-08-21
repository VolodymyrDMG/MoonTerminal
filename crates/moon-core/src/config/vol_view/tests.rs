use super::*;

/// EVERY field survives the TOML round trip used by vol_view.toml.
#[test]
fn vol_view_roundtrip_preserves_every_field() {
    let mut file = VolViewFile::default();
    file.view.enabled = false;
    file.view.height = VOL_HEIGHT_L;

    let text = toml::to_string_pretty(&file).expect("serialize");
    let back: VolViewFile = toml::from_str(&text).expect("parse");
    assert_eq!(back, file);
}

/// A partial or empty file completes with defaults — most importantly `enabled = true`.
#[test]
fn vol_view_partial_toml_fills_defaults() {
    let file: VolViewFile = toml::from_str("[view]\nheight = 0\n").expect("partial parse");
    assert!(file.view.enabled);
    assert_eq!(file.view.height, VOL_HEIGHT_S);

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
