//! Serializes complete ClientSettings mutations and manual orders for one core.
//!
//! MoonProto sends ClientSettings as a singleton full snapshot. A later packet can replace an
//! earlier pending packet, so every producer must share this queue. Manual orders form barriers:
//! the order is released only after the core echoes the exact visible group settings, and a later
//! settings generation cannot overtake it.

use std::collections::VecDeque;

use moonproto::MoonClient;

use super::convert::{apply_client_settings_edit, client_settings_from_proto};
use crate::config::{GroupExitSettings, TakeProfitMode};
use crate::feed::{trade, ClientSettingsEdit};

/// One manual order waiting behind its group-settings generation.
#[derive(Clone, Debug)]
pub(super) struct ManualOrder {
    /// Target market on the selected core.
    pub market: String,
    /// Position side.
    pub short: bool,
    /// Entry price.
    pub price: f64,
    /// Base-currency quantity already converted from the visible USD equivalent.
    pub size: f64,
    /// Optional core-owned strategy.
    pub strategy_id: Option<u64>,
    /// Visible settings that must be confirmed before placement.
    pub exit: GroupExitSettings,
}

/// A targeted mutation of the complete ClientSettings snapshot.
#[derive(Clone, Debug)]
enum SettingsMutation {
    /// Existing targeted toolbar/core-settings edit.
    Edit(ClientSettingsEdit),
    /// Blacklist flag and text, formerly sent through an independent full-snapshot path.
    Blacklist { on: bool, text: String },
    /// FORK (#63): put one market on the TEMPORARY blacklist for `remaining_days`, replacing any
    /// standing row for the same symbol.
    TempBan { symbol: String, remaining_days: f64 },
    /// FORK (#63): drop one market from the temporary blacklist.
    TempUnban { symbol: String },
    /// Complete visible group exit state.
    GroupExit(GroupExitSettings),
}

/// FORK (#63): how much a standing temp-ban countdown may fall short of the asked time and still
/// read as THIS ban, in days.
///
/// The core counts the rows down between our send and its echo, so exact equality never holds;
/// two minutes generously covers echo latency while staying far under the shortest preset.
const TEMP_BAN_SLACK_DAYS: f64 = 120.0 / 86_400.0;

/// One queue operation; an order is a serialization barrier between settings generations.
#[derive(Clone, Debug)]
enum SequenceOp {
    /// Idempotent settings mutation.
    Mutation(SettingsMutation),
    /// Manual order released only after every preceding mutation is echoed.
    Order(ManualOrder),
}

/// Pure next action selected from a retained settings snapshot.
enum SequenceAction {
    /// No work is currently possible or required.
    Idle,
    /// Send this complete snapshot and confirm the listed prefix mutation count on echo.
    Send {
        settings: moonproto::ClientSettingsCommand,
        mutation_count: usize,
    },
    /// Place one order, then continue planning against the same confirmed snapshot.
    Place(ManualOrder),
}

/// Per-core serializer for every complete ClientSettings write and gated manual order.
pub(in crate::feed) struct ClientSettingsSequence {
    queue: VecDeque<SequenceOp>,
    waiting_for_echo: bool,
    pending_confirmation: Option<(crate::feed::ClientSettings, usize)>,
}

impl ClientSettingsSequence {
    /// Create an empty serializer for a newly connected client.
    pub(in crate::feed) fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            waiting_for_echo: false,
            pending_confirmation: None,
        }
    }

    /// Retain queued work but forget connection-local send state before a reconnect attempt.
    pub(in crate::feed) fn prepare_reconnect(&mut self) {
        self.waiting_for_echo = false;
        self.pending_confirmation = None;
    }

    /// Queue a targeted ClientSettings edit without dropping it when no snapshot exists yet.
    pub(super) fn enqueue_edit(&mut self, edit: ClientSettingsEdit) {
        self.queue
            .push_back(SequenceOp::Mutation(SettingsMutation::Edit(edit)));
    }

    /// Queue a blacklist edit through the same full-snapshot serializer.
    pub(super) fn enqueue_blacklist(&mut self, on: bool, text: String) {
        self.queue
            .push_back(SequenceOp::Mutation(SettingsMutation::Blacklist {
                on,
                text,
            }));
    }

    /// FORK (#63): queue a temporary ban through the same serializer.
    pub(super) fn enqueue_temp_ban(&mut self, symbol: String, remaining_secs: f64) {
        self.queue
            .push_back(SequenceOp::Mutation(SettingsMutation::TempBan {
                symbol,
                remaining_days: (remaining_secs / 86_400.0).max(0.0),
            }));
    }

    /// FORK (#63): queue a temporary-ban removal through the same serializer.
    pub(super) fn enqueue_temp_unban(&mut self, symbol: String) {
        self.queue
            .push_back(SequenceOp::Mutation(SettingsMutation::TempUnban { symbol }));
    }

    /// Queue a proactive group-settings synchronization.
    pub(super) fn enqueue_group_exit(&mut self, exit: GroupExitSettings) {
        self.queue
            .push_back(SequenceOp::Mutation(SettingsMutation::GroupExit(exit)));
    }

    /// Queue an order behind its complete group-exit generation.
    pub(super) fn enqueue_order(&mut self, order: ManualOrder) {
        self.queue
            .push_back(SequenceOp::Mutation(SettingsMutation::GroupExit(
                order.exit,
            )));
        self.queue.push_back(SequenceOp::Order(order));
    }

    /// Record a successful full-snapshot send so later commands cannot overtake its echo.
    fn observe_send_success(
        &mut self,
        settings: &moonproto::ClientSettingsCommand,
        mutation_count: usize,
    ) {
        self.waiting_for_echo = true;
        self.pending_confirmation = Some((client_settings_from_proto(settings), mutation_count));
    }

    /// Allow the next plan after a ClientSettingsUpdated echo.
    pub(super) fn observe_update(&mut self) {
        self.waiting_for_echo = false;
    }

    /// Drive queued settings and orders against the client's retained snapshot.
    ///
    /// Returns whether at least one order was submitted so the caller may publish a best-effort
    /// order snapshot immediately.
    pub(super) fn drive(&mut self, client: &MoonClient, server_id: u64) -> bool {
        let Some(settings) = client
            .snapshot()
            .and_then(|snapshot| snapshot.settings().client_settings.clone())
        else {
            return false;
        };

        let mut order_submitted = false;
        loop {
            match self.next_action(&settings) {
                SequenceAction::Idle => return order_submitted,
                SequenceAction::Send {
                    settings: next,
                    mutation_count,
                } => {
                    match client.settings().send(next.clone()) {
                        Ok(()) => {
                            self.observe_send_success(&next, mutation_count);
                            log::info!(
                                "core {} serialized client settings sent",
                                crate::feed::core_label(server_id)
                            );
                        }
                        Err(error) => {
                            log::warn!(
                                "core {} serialized client settings failed: {error}",
                                crate::feed::core_label(server_id)
                            );
                        }
                    }
                    return order_submitted;
                }
                SequenceAction::Place(order) => {
                    trade::place_order(
                        client,
                        server_id,
                        order.market,
                        order.short,
                        order.price,
                        order.size,
                        order.strategy_id,
                        order.exit.use_stop_market,
                    );
                    order_submitted = true;
                }
            }
        }
    }

    /// Select the next deterministic action and discard mutations already reflected by the core.
    fn next_action(&mut self, settings: &moonproto::ClientSettingsCommand) -> SequenceAction {
        if self.waiting_for_echo {
            return SequenceAction::Idle;
        }
        if let Some((expected, mutation_count)) = self.pending_confirmation.take() {
            if client_settings_from_proto(settings) == expected {
                for _ in 0..mutation_count {
                    if !matches!(self.queue.front(), Some(SequenceOp::Mutation(_))) {
                        break;
                    }
                    self.queue.pop_front();
                }
            }
        }
        loop {
            match self.queue.front() {
                Some(SequenceOp::Mutation(mutation)) if mutation_satisfied(settings, mutation) => {
                    self.queue.pop_front();
                }
                Some(SequenceOp::Mutation(_)) => {
                    let mut next = settings.clone();
                    let mut mutation_count = 0;
                    for op in &self.queue {
                        let SequenceOp::Mutation(mutation) = op else {
                            break;
                        };
                        apply_mutation(&mut next, mutation);
                        mutation_count += 1;
                    }
                    return SequenceAction::Send {
                        settings: next,
                        mutation_count,
                    };
                }
                Some(SequenceOp::Order(order)) => {
                    if client_settings_from_proto(settings).group_exit_settings() != order.exit {
                        return SequenceAction::Idle;
                    }
                    let Some(SequenceOp::Order(order)) = self.queue.pop_front() else {
                        unreachable!("front was checked as an order");
                    };
                    return SequenceAction::Place(order);
                }
                None => return SequenceAction::Idle,
            }
        }
    }
}

/// Apply one idempotent mutation to a complete retained settings snapshot.
fn apply_mutation(settings: &mut moonproto::ClientSettingsCommand, mutation: &SettingsMutation) {
    match mutation {
        SettingsMutation::Edit(edit) => apply_client_settings_edit(settings, *edit),
        SettingsMutation::Blacklist { on, text } => {
            settings.use_coins_black_list = *on;
            settings.coins_black_list_text.clone_from(text);
        }
        SettingsMutation::TempBan {
            symbol,
            remaining_days,
        } => {
            // Replace-not-append: MoonBot keeps ONE timer per symbol, and re-banning restarts it.
            let mut rows = temp_rows_without(settings, symbol);
            rows.push((
                symbol.clone(),
                std::time::Duration::from_secs_f64((remaining_days * 86_400.0).max(0.0)),
            ));
            settings.set_temp_blacklist_entries(rows);
        }
        SettingsMutation::TempUnban { symbol } => {
            let rows = temp_rows_without(settings, symbol);
            settings.set_temp_blacklist_entries(rows);
        }
        SettingsMutation::GroupExit(exit) => apply_group_exit_settings(settings, *exit),
    }
}

/// FORK (#63): every temp-blacklist row except `symbol`'s, in the public setter's
/// `(symbol, remaining)` shape. Case-insensitive like the core's own list matching.
fn temp_rows_without(
    settings: &moonproto::ClientSettingsCommand,
    symbol: &str,
) -> Vec<(String, std::time::Duration)> {
    settings
        .temp_blacklist_entries()
        .filter(|row| !row.symbol.eq_ignore_ascii_case(symbol))
        .map(|row| (row.symbol.to_string(), row.remaining_duration()))
        .collect()
}

/// FORK (#63): the standing temp-ban countdown for `symbol`, in days, if the list has the row.
fn temp_remaining_days(settings: &moonproto::ClientSettingsCommand, symbol: &str) -> Option<f64> {
    settings
        .temp_blacklist_entries()
        .find(|row| row.symbol.eq_ignore_ascii_case(symbol))
        .map(|row| row.remaining_days())
}

/// Return whether applying a mutation would leave all projected settings unchanged.
fn mutation_satisfied(
    settings: &moonproto::ClientSettingsCommand,
    mutation: &SettingsMutation,
) -> bool {
    // FORK (#63): the temp rows are DELIBERATELY outside the `ClientSettings` projection (their
    // countdown would wedge the echo comparison), so the generic projection rule below would call
    // every temp mutation satisfied before it was ever sent. Each answers its real question
    // instead: does a standing row already say what this mutation asks?
    match mutation {
        SettingsMutation::TempBan {
            symbol,
            remaining_days,
        } => {
            // Satisfied only by a countdown that IS this ban, a beat behind at most: a LONGER
            // standing ban is not it (a re-ban deliberately shortens), and a shorter one has to
            // be re-armed.
            return temp_remaining_days(settings, symbol).is_some_and(|standing| {
                standing <= *remaining_days && standing >= *remaining_days - TEMP_BAN_SLACK_DAYS
            });
        }
        SettingsMutation::TempUnban { symbol } => {
            return temp_remaining_days(settings, symbol).is_none();
        }
        _ => {}
    }
    let before = client_settings_from_proto(settings);
    let mut after = settings.clone();
    apply_mutation(&mut after, mutation);
    before == client_settings_from_proto(&after)
}

/// Patch every visible group exit field while preserving invisible core-owned settings.
fn apply_group_exit_settings(
    settings: &mut moonproto::ClientSettingsCommand,
    exit: GroupExitSettings,
) {
    match exit.take_profit_mode {
        TakeProfitMode::Scalp => {
            apply_client_settings_edit(
                settings,
                ClientSettingsEdit::ScalpTakeProfit(exit.take_profit_pct),
            );
        }
        TakeProfitMode::Normal => {
            apply_client_settings_edit(
                settings,
                ClientSettingsEdit::TakeProfit {
                    pct: exit.take_profit_pct,
                    extended: false,
                },
            );
        }
        TakeProfitMode::Extended => {
            apply_client_settings_edit(
                settings,
                ClientSettingsEdit::TakeProfit {
                    pct: exit.take_profit_pct,
                    extended: true,
                },
            );
        }
    }
    for (index, pct) in exit.fixed_sell_pcts.into_iter().enumerate() {
        apply_client_settings_edit(
            settings,
            ClientSettingsEdit::SetFixedSellPct {
                slot: index + 1,
                pct,
            },
        );
    }
    if let Some(slot) = exit.fixed_sell_slot {
        apply_client_settings_edit(settings, ClientSettingsEdit::SelectFixedSellSlot(slot));
    } else {
        apply_client_settings_edit(settings, ClientSettingsEdit::EngageMainTakeProfit);
    }
    apply_client_settings_edit(
        settings,
        ClientSettingsEdit::StopLossPct(exit.stop_loss_pct),
    );
    apply_client_settings_edit(
        settings,
        ClientSettingsEdit::PanicIfPriceDrop(exit.stop_loss_enabled),
    );
    apply_client_settings_edit(
        settings,
        ClientSettingsEdit::UseStopMarket(exit.use_stop_market),
    );
}

#[cfg(test)]
mod tests;
