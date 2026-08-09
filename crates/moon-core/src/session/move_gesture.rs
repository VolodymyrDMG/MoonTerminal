//! Target selection for order-book Move gestures (Moonbot MultiOrders Move clicks).
//!
//! The chart layer resolves a click into a leg kind, a position side, and a click price; this
//! module owns the pure decision of which order legs move and where. It is kept UI-free so the
//! rules stay unit-testable and match Moonbot: the whole grid shifts by one delta anchored at
//! the leg nearest to the click, or only that nearest leg moves when grid moving is off.

/// One movable order leg: the order UID and the leg's current price.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveCandidate {
    pub uid: u64,
    pub price: f64,
}

/// One selected move: the order UID and the destination price for `move_order`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveCommand {
    pub uid: u64,
    pub new_price: f64,
}

/// Selects the order legs to move for a Move-gesture click at `click_price`.
///
/// The anchor is the candidate nearest to the click; a distance tie resolves to the lower UID so
/// the result does not depend on store iteration order. With `whole_grid` every candidate shifts
/// by `click_price - anchor.price`, preserving the grid's relative spacing (Moonbot's move-all
/// mode); the anchor itself lands exactly on the click price. Without it only the anchor moves,
/// directly to the click price.
///
/// Candidates with a non-finite or non-positive price are ignored, and shifted destinations that
/// would become non-positive are dropped, mirroring the shift-hotkey guard. A click on the
/// anchor's own price returns nothing: repricing to the same value would only churn the core.
/// The no-op epsilon matches the drag-settle guard in the chart's order dragging.
pub fn select_moves(
    candidates: &[MoveCandidate],
    click_price: f64,
    whole_grid: bool,
) -> Vec<MoveCommand> {
    if !click_price.is_finite() || click_price <= 0.0 {
        return Vec::new();
    }
    let mut anchor: Option<MoveCandidate> = None;
    for candidate in candidates {
        if !candidate.price.is_finite() || candidate.price <= 0.0 {
            continue;
        }
        let replace = match anchor {
            None => true,
            Some(current) => {
                let candidate_distance = (candidate.price - click_price).abs();
                let current_distance = (current.price - click_price).abs();
                candidate_distance < current_distance
                    || (candidate_distance == current_distance && candidate.uid < current.uid)
            }
        };
        if replace {
            anchor = Some(*candidate);
        }
    }
    let Some(anchor) = anchor else {
        return Vec::new();
    };
    let delta = click_price - anchor.price;
    if delta.abs() <= anchor.price.abs() * 1e-8 + 1e-8 {
        return Vec::new();
    }
    if whole_grid {
        candidates
            .iter()
            .filter(|candidate| candidate.price.is_finite() && candidate.price > 0.0)
            .map(|candidate| MoveCommand {
                uid: candidate.uid,
                new_price: candidate.price + delta,
            })
            .filter(|command| command.new_price > 0.0)
            .collect()
    } else {
        vec![MoveCommand {
            uid: anchor.uid,
            new_price: click_price,
        }]
    }
}

#[cfg(test)]
mod tests;
