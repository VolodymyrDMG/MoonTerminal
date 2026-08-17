//! `ChartTabs` ingestion of AddToChart detects, including creating and populating core tabs, plus
//! active-stack lookup by `(num, bucket)`. Extracted from `mod.rs`.

use gpui::*;

use super::{AddChartStack, ChartTabs, Tab};
use moon_core::config::ChartBucket;
use moon_core::session::CoreId;

impl ChartTabs {
    /// Ingest AddToChart detects with `add_to_chart > 0` by creating or populating a tab.
    ///
    /// The tab key is the core's `ChartBucket` (per-core, shared, or named bundle), resolved from
    /// core config and global `charts_split_by_core`. By default this does not switch `active` — a
    /// detect must not pull the user to a chart unasked. The `charts_auto_activate` setting opts
    /// in to BOTH signal-chart behaviors, without ever raising the OS window:
    /// - `SilentNoCharts=NO` detects open their coin on Main (newest per pass) and show Main;
    /// - the strip tab that received the newest AddToChart detect becomes active (detached
    ///   windows are skipped — their charts are already on screen).
    ///
    /// Args:
    ///     cx: Parent context used to read Backend detects and create or update stacks.
    ///
    /// Returns:
    ///     Nothing; new detects and stack observers are incorporated in place.
    pub(super) fn ingest(&mut self, cx: &mut Context<Self>) {
        let (split, auto_activate, fresh, open_main, cursors): (
            bool,
            bool,
            Vec<(u32, CoreId, ChartBucket, String, f64)>,
            Option<(f64, CoreId, String)>,
            Vec<(CoreId, u64)>,
        ) = {
            let b = self.backend.read(cx);
            let split = b.config.charts_split_by_core;
            let auto_activate = b.config.charts_auto_activate;
            let mut fresh = Vec::new();
            // Newest detect (by receipt time) whose strategy asked for a signal chart via
            // Moonbot's `SilentNoCharts=NO`; collected only when the opt-in setting is on.
            let mut open_main: Option<(f64, CoreId, String)> = None;
            let mut cursors = Vec::new();
            for s in b
                .session
                .sessions()
                .iter()
                .filter(|s| s.group == self.group)
            {
                let id = s.id;
                let Some(d) = b.session.store().core(id) else {
                    continue;
                };
                // Resolve the core bucket from its configured bundle and global split setting.
                // A core without config gets its own tab.
                let bucket = b
                    .config
                    .servers
                    .iter()
                    .find(|sv| sv.id == id)
                    .map(|sv| sv.chart_bucket(split))
                    .unwrap_or(ChartBucket::Core(id));
                let last = self.add_seq.get(&id).copied().unwrap_or(0);
                let mut mx = last;
                for det in &d.detects {
                    if det.seq <= last {
                        continue;
                    }
                    mx = mx.max(det.seq);
                    if det.add_to_chart > 0 {
                        let ttl = (det.keep_in_chart_secs.max(1) as f64) * 1000.0;
                        fresh.push((
                            det.add_to_chart,
                            id,
                            bucket.clone(),
                            det.market.clone(),
                            ttl,
                        ));
                    }
                    if auto_activate
                        && det.open_chart
                        && open_main.as_ref().is_none_or(|(t, _, _)| det.time_ms >= *t)
                    {
                        open_main = Some((det.time_ms, id, det.market.clone()));
                    }
                }
                if mx != last {
                    cursors.push((id, mx));
                }
            }
            (split, auto_activate, fresh, open_main, cursors)
        };
        for (id, mx) in cursors {
            self.add_seq.insert(id, mx);
        }
        // Moonbot signal charts (`SilentNoCharts=NO`): put the newest flagged coin on Main and
        // show Main, one open per pass — a burst re-focuses across passes instead of carpeting
        // the stack. `open_or_focus` reuses an existing Main panel for the same market. Runs
        // before the AddToChart branch, so when one detect asks for both, the Add tab (which
        // also received the coin) ends up in front. The OS window is never raised.
        if let Some((_, core, market)) = open_main {
            self.main.update(cx, |p, pcx| {
                // Default durable-history scope: a signal chart is a live open, not a Report row.
                p.open_or_focus(
                    core,
                    market,
                    crate::backend::ChartHistoryScope::Default,
                    pcx,
                );
            });
            if self.active != Tab::Main {
                self.active = Tab::Main;
                self.sync_inactive_chart_visibility(cx);
                self.sync_active_scale(cx);
            }
            self.sync_seen_for_active(cx);
        }
        if fresh.is_empty() {
            return;
        }
        // Detect diagnostics: AddToChart detects reached this group's UI. `fresh` is the number of
        // new AddToChart events processed in this pass. `channels.detect` enables this and is off
        // by default.
        moon_core::detect_diag::line(&format!(
            "[ingest] group={} split={split} fresh={} existing_tabs={}",
            self.group,
            fresh.len(),
            self.add.len()
        ));
        let (epoch, theme, backend, workspace_group) = (
            self.epoch,
            self.theme.clone(),
            self.backend.clone(),
            self.group.clone(),
        );
        // The newest detect that landed in a STRIP tab (not a detached window); with
        // `charts_auto_activate` on, that tab becomes active below.
        let mut last_strip_target: Option<(u32, ChartBucket)> = None;
        for (n, core, bucket, market, ttl) in fresh {
            let in_detached = self
                .detached
                .iter()
                .any(|(num, c, _)| *num == n && *c == bucket);
            if let Some((_, _, tab)) = self
                .add
                .iter()
                .find(|(num, c, _)| *num == n && *c == bucket)
                .or_else(|| {
                    self.detached
                        .iter()
                        .find(|(num, c, _)| *num == n && *c == bucket)
                })
            {
                if in_detached {
                    moon_core::detect_diag::line(&format!(
                        "[ingest] +coin n={n} bucket={bucket:?} market={market} → DETACHED-окно"
                    ));
                } else {
                    last_strip_target = Some((n, bucket.clone()));
                }
                tab.update(cx, |p, pcx| p.add_coin(core, &market, ttl, pcx));
            } else {
                let panel = cx.new(|_| {
                    AddChartStack::new(
                        backend.clone(),
                        workspace_group.clone(),
                        n,
                        bucket.clone(),
                        epoch,
                        theme.clone(),
                    )
                });
                // Restore this tab's saved per-tab display and comparison settings from charts.json.
                let (
                    saved_scale,
                    saved_layout,
                    saved_orderbook,
                    saved_liquidations,
                    saved_show_zone,
                    saved_auto_pin,
                    saved_orientation,
                    saved_action_pos,
                    saved_axis_pos,
                    saved_time_axis,
                    saved_line_labels,
                    saved_cursor_labels,
                    saved_candle_view,
                    saved_compare,
                ) = {
                    let specs = &self.backend.read(cx).chart_specs;
                    let spec = specs.iter().find(|s| s.matches(&self.group, n, &bucket));
                    (
                        spec.and_then(|s| s.scale),
                        spec.map_or((None, None, None), |s| {
                            (s.layout_mode, s.layout_height_fit, s.layout_height_scroll)
                        }),
                        spec.and_then(|s| s.orderbook_enabled),
                        spec.and_then(|s| s.liquidations_enabled),
                        spec.and_then(|s| s.show_zone),
                        spec.and_then(|s| s.auto_pin),
                        spec.and_then(|s| s.layout_orientation),
                        spec.map_or((None, None), |s| (s.cancel_buy_pos, s.panic_sell_pos)),
                        spec.and_then(|s| s.price_axis_pos),
                        spec.and_then(|s| s.time_axis_visible),
                        spec.and_then(|s| s.line_labels),
                        spec.and_then(|s| s.cursor_labels),
                        spec.and_then(|s| s.candle_view),
                        spec.map_or((None, false), |s| {
                            (s.compare_anchor.clone(), s.compare_orderbook_only)
                        }),
                    )
                };
                if saved_scale.is_some() {
                    panel.update(cx, |p, pcx| p.set_scale(saved_scale, pcx));
                }
                if saved_layout.0.is_some() || saved_layout.1.is_some() || saved_layout.2.is_some()
                {
                    panel.update(cx, |p, pcx| {
                        p.set_layout(saved_layout.0, saved_layout.1, saved_layout.2, pcx)
                    });
                }
                if saved_orderbook.is_some() {
                    panel.update(cx, |p, pcx| p.set_orderbook_enabled(saved_orderbook, pcx));
                }
                if saved_liquidations.is_some() {
                    panel.update(cx, |p, pcx| {
                        p.set_liquidations_enabled(saved_liquidations, pcx)
                    });
                }
                if saved_show_zone.is_some() {
                    panel.update(cx, |p, pcx| p.set_show_zone(saved_show_zone, pcx));
                }
                if saved_auto_pin.is_some() {
                    panel.update(cx, |p, pcx| p.set_auto_pin(saved_auto_pin, pcx));
                }
                if saved_orientation.is_some() {
                    panel.update(cx, |p, pcx| p.set_orientation(saved_orientation, pcx));
                }
                if saved_action_pos.0.is_some() || saved_action_pos.1.is_some() {
                    panel.update(cx, |p, pcx| {
                        p.set_action_btn_pos(saved_action_pos.0, saved_action_pos.1, pcx)
                    });
                }
                if saved_axis_pos.is_some() {
                    panel.update(cx, |p, pcx| p.set_price_axis_pos(saved_axis_pos, pcx));
                }
                if saved_time_axis.is_some() {
                    panel.update(cx, |p, pcx| p.set_time_axis_visible(saved_time_axis, pcx));
                }
                if saved_line_labels.is_some() {
                    panel.update(cx, |p, pcx| p.set_line_labels(saved_line_labels, pcx));
                }
                if saved_cursor_labels.is_some() {
                    panel.update(cx, |p, pcx| p.set_cursor_labels(saved_cursor_labels, pcx));
                }
                if saved_candle_view.is_some() {
                    panel.update(cx, |p, pcx| p.set_candle_view(saved_candle_view, pcx));
                }
                if saved_compare.0.is_some() || saved_compare.1 {
                    panel.update(cx, |p, pcx| {
                        p.restore_compare(saved_compare.0.clone(), saved_compare.1, pcx)
                    });
                }
                panel.update(cx, |p, pcx| p.add_coin(core, &market, ttl, pcx));
                self.watch_regular_stack_target(&panel, cx);
                self.add.push((n, bucket.clone(), panel));
                // Order tabs by `(number, bucket)`, matching egui's `sort_by_key` behavior.
                self.add.sort_by_key(|(num, c, _)| (*num, c.clone()));
                moon_core::detect_diag::line(&format!(
                    "[ingest] NEW tab n={n} bucket={bucket:?} (total_tabs={})",
                    self.add.len()
                ));
                // `active` does not change here; the opt-in switch happens once, below.
                last_strip_target = Some((n, bucket.clone()));
            }
        }
        // Opt-in auto-activation (`charts_auto_activate`): show the tab of the newest AddToChart
        // detect. Only the visible tab selection changes — the OS window is never raised, so the
        // user is not yanked across Spaces mid-typing.
        if auto_activate {
            if let Some((n, bucket)) = last_strip_target {
                let tab = Tab::Add(n, bucket);
                if self.active != tab {
                    self.active = tab;
                    self.sync_inactive_chart_visibility(cx);
                    self.sync_active_scale(cx);
                }
            }
        }
        self.sync_seen_for_active(cx);
        self.persist_scales(cx);
    }

    pub(super) fn add_stack(&self, n: u32, bucket: &ChartBucket) -> Option<Entity<AddChartStack>> {
        self.add
            .iter()
            .chain(self.custom.iter())
            .find(|(num, c, _)| *num == n && c == bucket)
            .map(|(_, _, p)| p.clone())
    }
}
