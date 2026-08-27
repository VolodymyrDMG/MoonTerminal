use super::book_zone_below_captions;

/// The gate's whole promise (#62): the clickable strip IS the painted book, translated into the
/// chart's own pixels, with the caption band taken off its top.
///
/// Plausible breakage: mixing the three coordinate spaces. The book arrives in window DEVICE
/// pixels, the caption bottom in window LOGICAL pixels, and the answer must be in slot-local
/// device pixels — a missed `* sf` or a missed origin subtraction moves the zone by exactly the
/// panel's position or the display's scale, which on a 1.5× display is most of the book.
#[test]
fn book_zone_subtracts_origin_and_scales_the_caption_bottom() {
    // Slot at (100, 50) device; book strip at device (400, 50), 200×600; scale 2.0.
    let zone = book_zone_below_captions(
        [400.0, 50.0, 200.0, 600.0],
        [100.0, 50.0],
        // Caption stack ends at 75 window-LOGICAL px → 150 device → slot-local y = 100.
        Some(75.0),
        2.0,
    )
    .expect("a painted book with room below the captions");

    assert_eq!(
        (zone.x, zone.y, zone.w, zone.h),
        (300.0, 100.0, 200.0, 500.0)
    );
}

/// No painted book — hidden, or the pane never prepared — must answer `None`, not a zero-width
/// rectangle a `contains` test would treat as a line the pointer can land on.
#[test]
fn an_unpainted_book_yields_no_zone() {
    assert!(book_zone_below_captions([0.0, 0.0, 0.0, 0.0], [0.0, 0.0], None, 1.0).is_none());
    // Height can be zero independently of width on a pane mid-layout.
    assert!(book_zone_below_captions([10.0, 0.0, 200.0, 0.0], [0.0, 0.0], None, 1.0).is_none());
}

/// With no captions drawn over the book, the whole painted strip trades.
#[test]
fn no_captions_leaves_the_whole_strip() {
    let zone =
        book_zone_below_captions([400.0, 0.0, 220.0, 800.0], [0.0, 0.0], None, 1.0).unwrap();
    assert_eq!((zone.y, zone.h), (0.0, 800.0));
}

/// A caption stack that swallows the strip — a cramped broom pane wrapping a long detect line —
/// leaves NOTHING clickable rather than a negative-height band below the book.
#[test]
fn captions_covering_the_book_leave_no_zone() {
    assert!(
        book_zone_below_captions([400.0, 0.0, 220.0, 100.0], [0.0, 0.0], Some(120.0), 1.0)
            .is_none()
    );
}

/// A caption bottom ABOVE the book's top must not extend the zone upward past the painted book:
/// the max() with the book's own top is what pins it.
#[test]
fn a_caption_above_the_book_does_not_grow_the_zone() {
    let zone =
        book_zone_below_captions([400.0, 200.0, 220.0, 400.0], [0.0, 0.0], Some(50.0), 1.0)
            .unwrap();
    assert_eq!((zone.y, zone.h), (200.0, 400.0));
}
