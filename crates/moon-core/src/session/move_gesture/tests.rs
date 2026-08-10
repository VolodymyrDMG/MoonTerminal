use super::*;

fn candidate(uid: u64, price: f64) -> MoveCandidate {
    MoveCandidate { uid, price }
}

#[test]
fn empty_candidates_move_nothing() {
    assert!(select_moves(&[], 10.0, true, true).is_empty());
    assert!(select_moves(&[], 10.0, false, true).is_empty());
}

#[test]
fn invalid_click_price_moves_nothing() {
    let grid = [candidate(1, 10.0)];
    assert!(select_moves(&grid, f64::NAN, true, true).is_empty());
    assert!(select_moves(&grid, 0.0, true, true).is_empty());
    assert!(select_moves(&grid, -3.0, true, true).is_empty());
}

#[test]
fn single_order_snaps_to_click() {
    let moves = select_moves(&[candidate(7, 100.0)], 97.5, true, true);
    assert_eq!(
        moves,
        vec![MoveCommand {
            uid: 7,
            new_price: 97.5
        }]
    );
}

#[test]
fn below_market_grid_anchors_at_its_highest_leg() {
    // A long entry grid sits below the market, so the head is the highest leg (100). The click
    // at 95 lands the head there and the others keep their one-unit distances — even though the
    // click is closest to the 98 leg.
    let grid = [candidate(3, 98.0), candidate(1, 100.0), candidate(2, 99.0)];
    let moves = select_moves(&grid, 95.0, true, true);
    assert_eq!(
        moves,
        vec![
            MoveCommand {
                uid: 3,
                new_price: 93.0
            },
            MoveCommand {
                uid: 1,
                new_price: 95.0
            },
            MoveCommand {
                uid: 2,
                new_price: 94.0
            },
        ]
    );
}

#[test]
fn above_market_grid_anchors_at_its_lowest_leg() {
    // A long TP grid sits above the market, so the head is the lowest leg (110).
    let grid = [
        candidate(1, 110.0),
        candidate(2, 112.0),
        candidate(3, 115.0),
    ];
    let moves = select_moves(&grid, 111.0, true, false);
    assert_eq!(
        moves,
        vec![
            MoveCommand {
                uid: 1,
                new_price: 111.0
            },
            MoveCommand {
                uid: 2,
                new_price: 113.0
            },
            MoveCommand {
                uid: 3,
                new_price: 116.0
            },
        ]
    );
}

#[test]
fn after_fills_the_next_remaining_leg_becomes_the_head() {
    // The former head at 100 filled and is no longer a candidate; the remaining grid re-anchors
    // at 99 and keeps its own spacing.
    let grid = [candidate(2, 99.0), candidate(3, 98.0)];
    let moves = select_moves(&grid, 95.0, true, true);
    assert_eq!(
        moves,
        vec![
            MoveCommand {
                uid: 2,
                new_price: 95.0
            },
            MoveCommand {
                uid: 3,
                new_price: 94.0
            },
        ]
    );
}

#[test]
fn head_price_tie_resolves_to_lower_uid() {
    let grid = [candidate(9, 100.0), candidate(4, 100.0)];
    let moves = select_moves(&grid, 95.0, true, true);
    // Both legs shift by the same delta; the tie only fixes which one is the no-op anchor when
    // the click hits the head price, so here both still move.
    assert_eq!(
        moves,
        vec![
            MoveCommand {
                uid: 9,
                new_price: 95.0
            },
            MoveCommand {
                uid: 4,
                new_price: 95.0
            },
        ]
    );
}

#[test]
fn grid_drops_non_positive_destinations() {
    // The head (100) lands on the click; the far leg at 30 would land at -10 and is dropped.
    let grid = [candidate(1, 100.0), candidate(2, 30.0)];
    let moves = select_moves(&grid, 60.0, true, true);
    assert_eq!(
        moves,
        vec![MoveCommand {
            uid: 1,
            new_price: 60.0
        }]
    );
}

#[test]
fn single_mode_moves_only_the_leg_nearest_to_the_click() {
    let grid = [candidate(1, 100.0), candidate(2, 98.0)];
    let moves = select_moves(&grid, 97.0, false, true);
    assert_eq!(
        moves,
        vec![MoveCommand {
            uid: 2,
            new_price: 97.0
        }]
    );
}

#[test]
fn single_mode_distance_tie_resolves_to_lower_uid() {
    let grid = [candidate(9, 101.0), candidate(4, 99.0)];
    let moves = select_moves(&grid, 100.0, false, true);
    assert_eq!(
        moves,
        vec![MoveCommand {
            uid: 4,
            new_price: 100.0
        }]
    );
}

#[test]
fn click_on_the_head_price_is_a_noop() {
    let grid = [candidate(1, 50.0), candidate(2, 49.0)];
    assert!(select_moves(&grid, 50.0, true, true).is_empty());
    assert!(select_moves(&grid, 50.0, false, true).is_empty());
}

#[test]
fn non_positive_and_non_finite_candidate_prices_are_ignored() {
    let grid = [
        candidate(1, 0.0),
        candidate(2, -5.0),
        candidate(3, f64::NAN),
        candidate(4, 20.0),
    ];
    let moves = select_moves(&grid, 18.0, true, true);
    assert_eq!(
        moves,
        vec![MoveCommand {
            uid: 4,
            new_price: 18.0
        }]
    );
}
