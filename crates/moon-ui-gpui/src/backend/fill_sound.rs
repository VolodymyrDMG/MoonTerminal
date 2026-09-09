//! FORK (#64): a sound when a MANUAL order EXECUTES — «только мои ручные ордера».
//!
//! Runs beside `detect_sound` on the same feed-drain wake and by the same rules: gated per core
//! on the orders revision, transition-driven so only a change OBSERVED here beeps — an order
//! first seen already executed (startup, reconnect, another terminal's history) is recorded
//! silently. Manual means `strat_id == 0`, the same reading the Orders table and the order-edit
//! window use: an order no strategy placed. Playback goes through the SAME embedded-WAV path the
//! detect and alert sounds use (`media::sound`), so the setting picks from the sounds the user
//! already knows, and a machine that can play alerts can play this.

use moon_core::session::CoreId;

use crate::Backend;

/// Per-core watch state: the orders revision last scanned, and each manual order's phase class.
#[derive(Default)]
pub(crate) struct FillWatch {
    /// `orders_table_rev` at the previous scan; a matching value skips the core entirely.
    rev: Option<u64>,
    /// Last phase class per order uid, pruned to the uids the snapshot still carries.
    phase: std::collections::HashMap<u64, u8>,
}

/// Phase classes of Moonbot's order `status`, in lifecycle order.
///
/// A CLASS rather than the raw string, so "did it execute" is one integer comparison and an
/// unknown spelling folds to "other" instead of faking a transition.
fn phase_class(status: &str) -> u8 {
    match status {
        // A fresh manual buy sits in `None` until the core's worker advances it — the same pair
        // the cancel-buys gate and the shift-hotkey guard accept as "a live buy".
        "None" | "BuySet" => 1,
        "BuyDone" => 2,
        "SellSet" => 3,
        // A partially filled sell: some of it EXECUTED.
        "SellAlmostDone" => 4,
        "SellDone" => 5,
        _ => 0,
    }
}

/// Whether moving between two phase classes is an EXECUTION the trader asked to hear.
///
/// Buy fills: a live buy (1) reaching `BuyDone` (2) — or jumping straight into a sell phase
/// (3..=5), which happens when the fill and the exit order land inside one drain tick. Sell
/// fills: a set sell (3) reaching its first partial (4) or done (5), and a partial reaching done.
fn transition_beeps(prev: u8, next: u8) -> bool {
    match (prev, next) {
        (1, 2..=5) => true,
        (3, 4 | 5) => true,
        (4, 5) => true,
        _ => false,
    }
}

impl Backend {
    /// Scan order snapshots for manual fills and play the configured sound on new ones.
    ///
    /// Called from the feed drain beside `play_detect_sounds`. The watch state advances even
    /// while the setting is off, so enabling it mid-session cannot replay executions that
    /// happened silent.
    pub(crate) fn play_fill_sounds(&mut self) {
        let cfg = self.preview.as_ref().unwrap_or(&self.config);
        let (enabled, sound) = (cfg.fill_sound_on, cfg.fill_sound.clone());
        let mut fired = false;
        let ids: Vec<CoreId> = self.session.sessions().iter().map(|s| s.id).collect();
        for core in ids {
            let Some(cd) = self.session.store().core(core) else {
                continue;
            };
            let watch = self.fill_watch.entry(core).or_default();
            if watch.rev == Some(cd.orders_table_rev) {
                continue;
            }
            let first_visit = watch.rev.is_none();
            watch.rev = Some(cd.orders_table_rev);
            let mut seen: std::collections::HashMap<u64, u8> =
                std::collections::HashMap::with_capacity(cd.orders.len());
            for row in &cd.orders {
                // Manual only: an order no strategy placed. Everything else is the core's own
                // trading, which already announces itself through detect sounds if asked.
                if row.strat_id != 0 {
                    continue;
                }
                let next = phase_class(&row.status);
                seen.insert(row.uid, next);
                if first_visit {
                    continue;
                }
                let Some(&prev) = watch.phase.get(&row.uid) else {
                    // First sight of this order. A NEW manual order normally appears as a live
                    // buy; appearing already executed means the fill predates our watch, and
                    // beeping for it would replay history.
                    continue;
                };
                if prev != next && transition_beeps(prev, next) {
                    fired = true;
                }
            }
            // Prune uids the snapshot no longer carries: a cancelled or archived order must not
            // leave a phase behind for a reused uid to trip over.
            watch.phase = seen;
        }
        if fired && enabled {
            crate::media::sound::play(&sound);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{phase_class, transition_beeps};

    /// The pinned status spellings: these come off the wire, and a renamed class here is a fill
    /// sound that silently stops firing.
    #[test]
    fn phase_classes_cover_the_lifecycle_in_order() {
        let order = [
            "None",
            "BuySet",
            "BuyDone",
            "SellSet",
            "SellAlmostDone",
            "SellDone",
        ];
        let classes: Vec<u8> = order.iter().map(|s| phase_class(s)).collect();
        assert_eq!(classes, [1, 1, 2, 3, 4, 5]);
        assert_eq!(phase_class("Cancelled"), 0, "unknown spellings are 'other'");
    }

    /// Executions beep; the rest of the lifecycle stays silent.
    ///
    /// The silent cases each guard a real hazard: an order first seen done is HISTORY (handled by
    /// the caller's first-sight rule); `BuyDone → SellSet` is the exit being PLACED, not filled;
    /// anything → 0 is a cancel or an unknown state, and a cancel is not a fill.
    #[test]
    fn only_executions_beep() {
        // Buy fills — including the jump past BuyDone when the exit lands in the same tick.
        assert!(transition_beeps(1, 2));
        assert!(transition_beeps(1, 3));
        assert!(transition_beeps(1, 5));
        // Sell fills, partial first.
        assert!(transition_beeps(3, 4));
        assert!(transition_beeps(3, 5));
        assert!(transition_beeps(4, 5));

        // Placing the exit is not a fill.
        assert!(!transition_beeps(2, 3));
        // A cancel is not a fill.
        assert!(!transition_beeps(1, 0));
        assert!(!transition_beeps(3, 0));
        // Standing still is nothing.
        assert!(!transition_beeps(3, 3));
        // Backwards (a core resync rewinding a status) must not read as an execution.
        assert!(!transition_beeps(4, 3));
        assert!(!transition_beeps(2, 1));
    }
}
