//! Static action-authority regressions for group-owned chart panels.

/// Removing a dispatch-time `workspace_action_allows_core` guard from `trade.rs`, `render.rs`, or
/// `render_input.rs` must fail: a stale chart could trade or navigate a core other than the Auto
/// rail selection.
#[test]
fn every_chart_command_and_navigation_path_revalidates_auto_authority() {
    let trade = include_str!("trade.rs");
    for command in [
        "manual chart order blocked",
        "chart start-cross cancel",
        "hotkey cancel hovered order failed",
        "hotkey split hovered order failed",
        "manual chart move line:",
    ] {
        let command_at = trade
            .find(command)
            .unwrap_or_else(|| panic!("missing chart action marker: {command}"));
        let prefix = &trade[..command_at];
        assert!(
            prefix
                .rfind("workspace_action_allows_core")
                .is_some_and(|guard_at| command_at - guard_at < 2_500),
            "{command} must follow a nearby live workspace-authority guard"
        );
    }
    let menu = trade
        .split("pub(super) fn try_open_order_menu(")
        .nth(1)
        .and_then(|tail| tail.split("pub fn cancel_hovered_order(").next())
        .expect("chart order-menu producer must remain present");
    assert!(
        menu.find("workspace_action_allowed").unwrap()
            < menu
                .find("workspace_group: self.workspace_group.clone()")
                .unwrap(),
        "the chart menu must validate before carrying group authority into delayed callbacks"
    );

    let render = include_str!("render.rs");
    let action_callback = render
        .split("fn action_button(")
        .nth(1)
        .and_then(|tail| tail.split("impl Render for ChartPanel").next())
        .expect("chart action-button callback must remain present");
    assert!(
        action_callback.contains("workspace_action_allows_core(workspace_group.as_deref(), core)")
            && action_callback
                .find("workspace_action_allows_core")
                .unwrap()
                < action_callback.find("cancel_market_buys").unwrap()
            && action_callback
                .find("workspace_action_allows_core")
                .unwrap()
                < action_callback.find("toggle_panic_sell").unwrap(),
        "Cancel Buy and Panic Sell must revalidate before dispatch"
    );

    let input = include_str!("render_input.rs");
    let navigation = input
        .split("if let Some((core, market)) = this.input.pending_to_main.take()")
        .nth(1)
        .and_then(|tail| tail.split("if input_changed || opened_to_main").next())
        .expect("AddToChart-to-Main navigation callback must remain present");
    assert!(
        navigation.find("workspace_action_allows_core").unwrap()
            < navigation.find("open_on_main").unwrap(),
        "old chart navigation must not bypass the Auto rail"
    );
}

/// Removing the group argument from `main_stack.rs` or `add_stack.rs` must fail: chart callbacks
/// would become explicitly unscoped and the live guards above would always allow stale cores.
#[test]
fn chart_stacks_pass_their_workspace_group_into_every_panel() {
    let main = include_str!("../../chart_tabs/main_stack.rs");
    let add = include_str!("../../chart_tabs/add_stack.rs");
    assert!(main.contains(
        "ChartPanel::new_main(\n                backend,\n                Some(workspace_group),"
    ));
    assert!(add.contains(
        "ChartPanel::new_addto(backend, workspace_group, num, bucket, epoch, theme, cx)"
    ));
}

/// The volume-measure gesture must occupy exactly its slot in the left-press priority chain:
/// AFTER figure handling (drawing keeps its modifier gestures over the band) and BEFORE the
/// trading gestures (inside the band a plain press measures — it must never place an order).
/// The release half must distinguish a drag (keep the bracket) from a stationary click (clear).
#[test]
fn volume_measure_sits_between_figures_and_trading_in_the_press_chain() {
    let source = include_str!("render_input.rs");
    let down = source
        .split("pub(super) fn mouse_down_left(")
        .nth(1)
        .and_then(|tail| tail.split("pub(super) fn mouse_down_right(").next())
        .expect("left-press router must exist");
    let probe = down
        .find("vol_measure_probe")
        .expect("band press probe must exist");
    let fig = down
        .find("try_fig_click")
        .expect("figure branch must exist");
    let trade = down
        .find("try_place_order_click")
        .expect("trading branch must exist");
    assert!(fig < probe, "figure layer keeps priority over the band");
    assert!(
        probe < trade,
        "a band press must never fall through to trading"
    );

    let up = source
        .split("pub(super) fn mouse_up_left(")
        .nth(1)
        .and_then(|tail| tail.split("pub(super) fn mouse_down_right(").next())
        .expect("left-release router must exist");
    assert!(up.contains("this.vol_measure_drag.take()"));
    assert!(
        up.contains("set_vol_measure(pane, None)"),
        "a stationary click must clear the bracket"
    );
}

/// The header readout chain must stay wired end to end: the render pass pushes the global
/// vol-view config into the engine, collects per-pane header data, and renders the overlay row.
/// Dropping any link silently loses the Bv/Sv block, the 24h delta, or the Ses figure.
#[test]
fn chart_header_readouts_stay_wired_from_config_to_overlay() {
    let render = include_str!("render.rs");
    assert!(render.contains("self.chart.set_vol_view(vol_view)"));
    assert!(render.contains("vol_header_data"));
    assert!(render.contains("super::vol_header::header_overlays"));

    let header = include_str!("vol_header.rs");
    // 24h must be the REAL day change (day_delta_pct behind the panel's 30s cache), NOT the
    // bot's coin_24h_delta average-deviation — the +0.8%-on-a-+13%-coin regression. Ses must
    // answer for THIS market from the per-market profit map, not for the whole core.
    assert!(header.contains("day_delta_pct(core, market)"));
    assert!(header.contains("const TTL_MS: f64 = 30_000.0;"));
    assert!(header.contains(".and_then(|d| d.market_profit.get(market).copied())"));
    assert!(render.contains("self.day_delta_cached(&src, core, &market, now_ms)"));
}
