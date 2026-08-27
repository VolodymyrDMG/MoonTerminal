use super::{
    client_settings_from_proto, ClientSettingsSequence, GroupExitSettings, ManualOrder,
    SequenceAction, TakeProfitMode,
};
use crate::feed::ClientSettingsEdit;

/// Build a valid visible settings generation for serializer tests.
fn exit_settings(tp: f64, stop_market: bool) -> GroupExitSettings {
    GroupExitSettings {
        take_profit_pct: tp,
        take_profit_mode: TakeProfitMode::Normal,
        fixed_sell_pcts: [2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
        fixed_sell_slot: Some(2),
        stop_loss_pct: -4.0,
        stop_loss_enabled: true,
        use_stop_market: stop_market,
    }
}

/// Extract the next full settings packet or fail with the unexpected action.
fn next_settings(
    sequence: &mut ClientSettingsSequence,
    snapshot: &moonproto::ClientSettingsCommand,
) -> moonproto::ClientSettingsCommand {
    match sequence.next_action(snapshot) {
        SequenceAction::Send { settings, .. } => settings,
        _ => panic!("expected a full ClientSettings send"),
    }
}

#[test]
/// Regression target: making `ClientSettingsSequence::next_action` apply only the newest mutation
/// drops an earlier ManualStrategy or blacklist edit, so the user's next order uses stale settings.
fn full_settings_mutations_compose_before_the_echo() {
    let base = moonproto::ClientSettingsCommand::default();
    let exit = exit_settings(12.0, true);
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_group_exit(exit);
    sequence.enqueue_edit(ClientSettingsEdit::ManualStrategy { on: true, id: 77 });
    sequence.enqueue_blacklist(true, "SCAM,TEST".to_string());

    let sent = next_settings(&mut sequence, &base);
    let projected = client_settings_from_proto(&sent);
    assert_eq!(projected.group_exit_settings(), exit);
    assert!(projected.use_manual_strategy);
    assert_eq!(projected.manual_strategy_id, 77);
    assert!(projected.use_blacklist);
    assert_eq!(projected.blacklist_text, "SCAM,TEST");

    sequence.observe_update();
    assert!(matches!(sequence.next_action(&sent), SequenceAction::Idle));
}

/// Regression target: removing the `x_tmode = false` write from
/// `feed::live::convert::apply_client_settings_edit` makes a Scalp generation inherit Extended
/// fixed-sell encoding, so its projected echo no longer equals the local S1-S6 values.
#[test]
fn scalp_generation_clears_extended_fixed_sell_encoding() {
    let mut base = moonproto::ClientSettingsCommand::default();
    base.x_tmode = true;
    let exit = GroupExitSettings {
        fixed_sell_pcts: [1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        ..GroupExitSettings::default()
    };
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_group_exit(exit);

    let sent = next_settings(&mut sequence, &base);

    assert!(!sent.x_tmode);
    assert_eq!(
        client_settings_from_proto(&sent).group_exit_settings(),
        exit
    );
}

/// Regression target: clearing `waiting_for_echo` in `enqueue_edit` lets a later full snapshot
/// overtake an unconfirmed group exit, so the later packet can erase the user's visible settings.
#[test]
fn commands_arriving_during_an_inflight_generation_wait_for_its_echo() {
    let base = moonproto::ClientSettingsCommand::default();
    let exit = exit_settings(12.0, true);
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_group_exit(exit);

    let first_sent = next_settings(&mut sequence, &base);
    sequence.observe_send_success(&first_sent, 1);
    sequence.enqueue_edit(ClientSettingsEdit::ManualStrategy { on: true, id: 77 });
    assert!(matches!(sequence.next_action(&base), SequenceAction::Idle));

    sequence.observe_update();
    let second_sent = next_settings(&mut sequence, &first_sent);
    let projected = client_settings_from_proto(&second_sent);
    assert_eq!(projected.group_exit_settings(), exit);
    assert!(projected.use_manual_strategy);
    assert_eq!(projected.manual_strategy_id, 77);
}

/// Regression target: confirming a composed packet without retiring its entire mutation prefix
/// leaves an older conflicting TP at the queue front and wedges every later order behind it.
#[test]
fn a_confirmed_packet_retires_all_mutations_it_composed() {
    let base = moonproto::ClientSettingsCommand::default();
    let first = exit_settings(10.0, false);
    let second = exit_settings(20.0, true);
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_group_exit(first);
    sequence.enqueue_group_exit(second);

    let sent = next_settings(&mut sequence, &base);
    sequence.observe_send_success(&sent, 2);
    sequence.observe_update();
    assert!(matches!(sequence.next_action(&sent), SequenceAction::Idle));
}

/// Regression target: retaining the connection-local echo wait across `live::run` reconnects leaves
/// queued edits permanently idle because the dropped client can no longer emit their confirmation.
#[test]
fn reconnect_retries_unconfirmed_mutations_without_losing_them() {
    let base = moonproto::ClientSettingsCommand::default();
    let exit = exit_settings(12.0, true);
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_group_exit(exit);

    let sent = next_settings(&mut sequence, &base);
    sequence.observe_send_success(&sent, 1);
    sequence.prepare_reconnect();

    let retried = next_settings(&mut sequence, &base);
    assert_eq!(
        client_settings_from_proto(&retried).group_exit_settings(),
        exit
    );
}

#[test]
/// Regression target: removing the order barrier in `ClientSettingsSequence::next_action` lets the
/// second chart generation overtake the first, so a rapid double-click receives another TP/SL.
fn orders_release_after_their_own_confirmed_generation() {
    let base = moonproto::ClientSettingsCommand::default();
    let first = exit_settings(10.0, false);
    let second = exit_settings(20.0, true);
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_order(ManualOrder {
        market: "ETHBTC".to_string(),
        short: false,
        price: 0.04,
        size: 0.25,
        strategy_id: None,
        exit: first,
    });
    sequence.enqueue_order(ManualOrder {
        market: "BTCUSDT".to_string(),
        short: true,
        price: 100_000.0,
        size: 0.001,
        strategy_id: None,
        exit: second,
    });

    let first_echo = next_settings(&mut sequence, &base);
    sequence.observe_update();
    match sequence.next_action(&first_echo) {
        SequenceAction::Place(order) => assert_eq!(order.exit, first),
        _ => panic!("first order did not release after its settings echo"),
    }
    let second_echo = next_settings(&mut sequence, &first_echo);
    sequence.observe_update();
    match sequence.next_action(&second_echo) {
        SequenceAction::Place(order) => assert_eq!(order.exit, second),
        _ => panic!("second order did not release after its settings echo"),
    }
}

/// FORK (#63): a temp ban composes into the outgoing snapshot as ONE row per symbol with the asked
/// countdown, replacing any standing row rather than stacking a second timer beside it.
#[test]
fn temp_ban_replaces_the_symbols_standing_row() {
    let mut base = moonproto::ClientSettingsCommand::default();
    base.set_temp_blacklist_entries([
        ("ADAUSDT".to_string(), std::time::Duration::from_secs(600)),
        ("DOGEUSDT".to_string(), std::time::Duration::from_secs(3_600)),
    ]);
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_temp_ban("adausdt".to_string(), 900.0);

    let sent = next_settings(&mut sequence, &base);
    let rows: Vec<(String, f64)> = sent
        .temp_blacklist_entries()
        .map(|row| (row.symbol.to_string(), row.remaining_days()))
        .collect();
    // The unrelated row survives untouched; the banned symbol carries the NEW countdown, once,
    // under the spelling the caller asked for.
    assert_eq!(rows.len(), 2, "{rows:?}");
    let doge = rows.iter().find(|(s, _)| s == "DOGEUSDT").expect("doge kept");
    assert!((doge.1 * 86_400.0 - 3_600.0).abs() < 1.0);
    let ada = rows.iter().find(|(s, _)| s == "adausdt").expect("ada rewritten");
    assert!((ada.1 * 86_400.0 - 900.0).abs() < 1.0, "countdown is the asked 15 minutes");
}

/// FORK (#63): the echoed countdown is a beat BEHIND the asked one, and that echo must retire the
/// mutation — while a snapshot with the symbol still missing, or still carrying the OLD longer
/// timer, must not.
#[test]
fn temp_ban_retires_on_a_counted_down_echo_only() {
    let base = moonproto::ClientSettingsCommand::default();
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_temp_ban("ADAUSDT".to_string(), 900.0);

    let sent = next_settings(&mut sequence, &base);
    sequence.observe_update();

    // The core echoes the row ten seconds shorter: this IS our ban, and the queue drains.
    let mut echo = sent.clone();
    echo.set_temp_blacklist_entries([(
        "ADAUSDT".to_string(),
        std::time::Duration::from_secs(890),
    )]);
    assert!(matches!(sequence.next_action(&echo), SequenceAction::Idle));

    // A NEW ban for the same symbol while a longer timer stands is not satisfied by it.
    sequence.enqueue_temp_ban("ADAUSDT".to_string(), 60.0);
    let resent = next_settings(&mut sequence, &echo);
    let row: Vec<f64> = resent
        .temp_blacklist_entries()
        .map(|r| r.remaining_days() * 86_400.0)
        .collect();
    assert_eq!(row.len(), 1);
    assert!((row[0] - 60.0).abs() < 1.0, "re-ban deliberately shortens the timer");
}

/// FORK (#63): an unban drops exactly the asked symbol and retires once the echo comes back bare.
#[test]
fn temp_unban_drops_the_row_and_retires_on_the_bare_echo() {
    let mut base = moonproto::ClientSettingsCommand::default();
    base.set_temp_blacklist_entries([
        ("ADAUSDT".to_string(), std::time::Duration::from_secs(600)),
        ("DOGEUSDT".to_string(), std::time::Duration::from_secs(3_600)),
    ]);
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_temp_unban("ADAUSDT".to_string());

    let sent = next_settings(&mut sequence, &base);
    let rows: Vec<String> = sent
        .temp_blacklist_entries()
        .map(|row| row.symbol.to_string())
        .collect();
    assert_eq!(rows, ["DOGEUSDT"]);

    sequence.observe_update();
    assert!(matches!(sequence.next_action(&sent), SequenceAction::Idle));

    // Unbanning a symbol that is already gone queues nothing to send at all.
    sequence.enqueue_temp_unban("ADAUSDT".to_string());
    assert!(matches!(sequence.next_action(&sent), SequenceAction::Idle));
}
