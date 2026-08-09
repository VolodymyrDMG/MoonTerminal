use super::*;

fn candidate(uid: u64, price: f64) -> MoveCandidate {
    MoveCandidate { uid, price }
}

#[test]
fn empty_candidates_move_nothing() {
    assert!(select_moves(&[], 10.0, true).is_empty());
    assert!(select_moves(&[], 10.0, false).is_empty());
}

#[test]
fn invalid_click_price_moves_nothing() {
    let grid = [candidate(1, 10.0)];
    assert!(select_moves(&grid, f64::NAN, true).is_empty());
    assert!(select_moves(&grid, 0.0, true).is_empty());
    assert!(select_moves(&grid, -3.0, true).is_empty());
}

#[test]
fn single_order_snaps_to_click() {
    let moves = select_moves(&[candidate(7, 100.0)], 97.5, true);
    assert_eq!(
        moves,
        vec![MoveCommand {
            uid: 7,
            new_price: 97.5
        }]
    );
}

#[test]
fn grid_shifts_by_anchor_delta_preserving_spacing() {
    // Click below the grid: the nearest leg is 98, so the whole grid shifts down by 3.
    let grid = [candidate(1, 100.0), candidate(2, 99.0), candidate(3, 98.0)];
    let moves = select_moves(&grid, 95.0, true);
    assert_eq!(
        moves,
        vec![
            MoveCommand {
                uid: 1,
                new_price: 97.0
            },
            MoveCommand {
                uid: 2,
                new_price: 96.0
            },
            MoveCommand {
                uid: 3,
                new_price: 95.0
            },
        ]
    );
}

#[test]
fn nearest_distance_tie_resolves_to_lower_uid() {
    let grid = [candidate(9, 101.0), candidate(4, 99.0)];
    let moves = select_moves(&grid, 100.0, false);
    assert_eq!(
        moves,
        vec![MoveCommand {
            uid: 4,
            new_price: 100.0
        }]
    );
}

#[test]
fn grid_drops_non_positive_destinations() {
    // The anchor (100) lands on the click; the far leg at 1 would land at -39 and is dropped.
    let grid = [candidate(1, 100.0), candidate(2, 1.0)];
    let moves = select_moves(&grid, 60.0, true);
    assert_eq!(
        moves,
        vec![MoveCommand {
            uid: 1,
            new_price: 60.0
        }]
    );
}

#[test]
fn single_mode_moves_only_the_nearest_leg() {
    let grid = [candidate(1, 100.0), candidate(2, 98.0)];
    let moves = select_moves(&grid, 97.0, false);
    assert_eq!(
        moves,
        vec![MoveCommand {
            uid: 2,
            new_price: 97.0
        }]
    );
}

#[test]
fn click_on_anchor_price_is_a_noop() {
    let grid = [candidate(1, 50.0), candidate(2, 49.0)];
    assert!(select_moves(&grid, 50.0, true).is_empty());
    assert!(select_moves(&grid, 50.0, false).is_empty());
}

#[test]
fn non_positive_and_non_finite_candidate_prices_are_ignored() {
    let grid = [
        candidate(1, 0.0),
        candidate(2, -5.0),
        candidate(3, f64::NAN),
        candidate(4, 20.0),
    ];
    let moves = select_moves(&grid, 18.0, true);
    assert_eq!(
        moves,
        vec![MoveCommand {
            uid: 4,
            new_price: 18.0
        }]
    );
}
