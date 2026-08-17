// Do not use `super::*`: the parent re-exports GPUI's `test` attribute macro through its imports,
// which would shadow the built-in `#[test]`.
use super::layout::us_letter;

/// Pins what the layout translation must NOT touch.
///
/// Plausible breakage: translating an ASCII name round-trips a letter through the active layout and
/// can return a DIFFERENT letter under a non-Latin one; translating a multi-character name would
/// take the first character of `f7` or `delete` and answer for it.
#[test]
fn only_single_non_ascii_key_names_are_translated() {
    for key in ["a", "z", "s", "f7", "delete", "escape", "up", "", "1"] {
        assert_eq!(us_letter(key), None, "{key:?} must be left alone");
    }
}

/// Pins the physical-key answer on Windows when the active layout IS Latin.
///
/// Plausible breakage: a translation that fires on ASCII would rewrite every letter hotkey on the
/// developer's own machine, which is exactly where it would go unnoticed.
#[test]
fn a_latin_layout_needs_no_translation() {
    // Whatever layout this machine runs, an ASCII name is already the physical key.
    assert_eq!(us_letter("q"), None);
    assert_eq!(us_letter("w"), None);
}

/// Pins that the two clicks of an armed Sells-to-zone draw ARE the two prices that go on the wire.
///
/// Plausible breakage: the Zone tool starts building its band from something other than its two
/// placed nodes — a snapped level, a single price plus a width — and the command silently addresses
/// a band the user did not draw, which nothing downstream can detect. Which TOOLS are bands is
/// pinned next to the tools themselves, in `moon_core::figures::tools`.
#[test]
fn the_zone_tool_sends_the_two_prices_that_were_clicked() {
    use moon_core::figures::{FigNode, FigureTool};

    let clicks = [FigNode::new(1_000.0, 42.5), FigNode::new(2_000.0, 41.0)];
    let def = FigureTool::Channel.def();
    assert_eq!(def.clicks, 2, "the mode is documented as a two-click draw");
    let kind = (def.make)(&clicks).expect("two nodes finish a Zone");
    assert_eq!(kind.price_band(), Some((42.5, 41.0)));
}

/// The shift-hotkey pre-check must accept every phase the trader's press can legitimately move.
///
/// Named regression: shipping this guard as `status == "BuySet"` alone made the shift keys read
/// as dead for the MOST common press — nudging a freshly placed limit buy, which the core still
/// holds in `None` until its worker advances it (the terminal's own cancel-buys gate accepts the
/// same `None | BuySet` pair). The sell side likewise must not drop a partially filled sell whose
/// remainder is a live order.
#[test]
fn shift_phase_guard_accepts_fresh_buys_and_partial_sells() {
    use super::shift_phase_matches;

    // Buy side: fresh (None) and set entries move; a filled entry no longer has a buy to shift.
    assert!(shift_phase_matches(false, "None"));
    assert!(shift_phase_matches(false, "BuySet"));
    assert!(!shift_phase_matches(false, "BuyDone"));
    assert!(!shift_phase_matches(false, "SellSet"));

    // Sell side: set and partially filled sells move; done and buy phases do not.
    assert!(shift_phase_matches(true, "SellSet"));
    assert!(shift_phase_matches(true, "SellAlmostDone"));
    assert!(!shift_phase_matches(true, "SellDone"));
    assert!(!shift_phase_matches(true, "BuySet"));
    assert!(!shift_phase_matches(true, "None"));
}
