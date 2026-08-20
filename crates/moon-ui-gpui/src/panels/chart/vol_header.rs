//! Chart header readouts mirrored from the bot's chart caption: the Bv/Sv block (buy/sell quote
//! volume over a click-configurable trailing window, with net delta and a pressure ratio bar),
//! the coin's 24-hour delta, and the core's session profit ("Ses").
//!
//! One overlay row per pane, anchored at the plot's top edge right of the pin/lock corner
//! controls. The window chip opens a small anchored menu (the user's pick over a blind cycling
//! click); the choice is global, stored in `vol_view.toml`, and feeds the same tape the volume
//! band draws, so the figures and the graph can never disagree about what a "buy" was.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, MoonText, rgba_from, v_flex};

use moon_core::session::CoreId;

use super::ChartPanel;
use crate::chartdx::volume_graph::format_quote_short;
use crate::design;

/// Offered Bv/Sv windows in seconds, rendered as the menu rows.
pub(super) const VOL_WINDOWS: [(u16, &str); 6] = [
    (10, "10s"),
    (30, "30s"),
    (60, "1m"),
    (300, "5m"),
    (900, "15m"),
    (3600, "1h"),
];

/// Label for a window value, falling back to a raw seconds form for hand-edited configs.
pub(super) fn window_label(secs: u32) -> String {
    VOL_WINDOWS
        .iter()
        .find(|(s, _)| u32::from(*s) == secs)
        .map(|(_, l)| (*l).to_string())
        .unwrap_or_else(|| format!("{secs}s"))
}

/// Resolved figures for one pane's header row.
pub(super) struct VolHeader {
    /// Pane index, carried into the menu-toggle click handler.
    pub pane: usize,
    /// Buy/sell quote volume over the configured window, when the tape holds any.
    pub bvsv: Option<(f32, f32)>,
    /// Signed 24-hour delta percent from the market's delta state (bot semantics: deviation
    /// from the retained daily average), when the market resolves.
    pub delta_24h: Option<f64>,
    /// Session profit in the core's quote currency, when the core has reported counters.
    pub ses: Option<f64>,
    /// Left plot edge and top edge in logical pixels.
    pub left: f32,
    pub top: f32,
    /// Right plot edge in logical pixels; the row clips against it.
    pub right: f32,
}

/// Signed-value color: positive green, negative red, zero neutral.
fn signed_color(p: &MoonPalette, v: f64) -> u32 {
    if v > 0.0 {
        p.green
    } else if v < 0.0 {
        p.red
    } else {
        p.text_soft
    }
}

/// Build the header row element for one pane, plus the window menu when it is open for this pane.
pub(super) fn header_element(
    entity: Entity<ChartPanel>,
    h: VolHeader,
    window_secs: u32,
    menu_open: bool,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let mono = |text: String, color: u32| {
        MoonText::new(text)
            .color(color)
            .mono(true)
            .uppercase(false)
            .render()
    };
    let mut row = div()
        .id(SharedString::from(format!("vol-header-{}", h.pane)))
        .absolute()
        .left(px(h.left))
        .top(px(h.top))
        .max_w(px((h.right - h.left - 8.0).max(60.0)))
        .flex()
        .flex_row()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .px(design::ui_px(cx, 4.0))
        .py(px(1.0))
        .rounded(design::ui_px(cx, 3.0))
        .bg(rgba_from(p.surface, 0.4));

    // Window chip: the one interactive piece — opens the preset menu below the row.
    {
        let entity = entity.clone();
        let pane = h.pane;
        row = row.child(
            div()
                .id(SharedString::from(format!("vol-win-chip-{}", h.pane)))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .px(design::ui_px(cx, 3.0))
                .rounded(design::ui_px(cx, 3.0))
                .cursor_pointer()
                .hover(|s| s.bg(rgba_from(p.panel_high, 0.9)))
                .child(mono(window_label(window_secs), p.text))
                .child(mono("▾".to_string(), p.text_soft))
                .on_click(move |_, _w, app| {
                    entity.update(app, |this, cx| {
                        this.vol_menu_pane = if this.vol_menu_pane == Some(pane) {
                            None
                        } else {
                            Some(pane)
                        };
                        cx.notify();
                    });
                }),
        );
    }

    if let Some((bv, sv)) = h.bvsv {
        let net = f64::from(bv) - f64::from(sv);
        row = row
            .child(mono(format!("Bv {}", format_quote_short(bv)), p.green))
            .child(mono(format!("Sv {}", format_quote_short(sv)), p.red))
            .child(mono(
                format!(
                    "Δ {}{}",
                    if net < 0.0 { "-" } else { "+" },
                    format_quote_short(net.abs() as f32)
                ),
                signed_color(&p, net),
            ));
        // Pressure ratio bar: buys vs sells share of the window, the bot's square-vs-bar read.
        let total = bv + sv;
        if total > 0.0 {
            const RATIO_W: f32 = 34.0;
            let buy_w = (RATIO_W * bv / total).clamp(1.0, RATIO_W - 1.0);
            row = row.child(
                div()
                    .w(px(RATIO_W))
                    .h(px(4.0))
                    .rounded(px(1.0))
                    .flex()
                    .flex_row()
                    .overflow_hidden()
                    .child(div().w(px(buy_w)).h_full().bg(rgb(p.green)))
                    .child(div().flex_1().h_full().bg(rgb(p.red))),
            );
        }
    }

    if let Some(d) = h.delta_24h {
        row = row.child(mono(format!("24h {d:+.1}%"), signed_color(&p, d)));
    }
    if let Some(ses) = h.ses {
        row = row.child(mono(format!("Ses {ses:+.2}$"), signed_color(&p, ses)));
    }

    if !menu_open {
        return row.into_any_element();
    }

    // The open window menu, anchored just below the row at its left edge.
    let mut menu = v_flex()
        .absolute()
        .left(px(h.left))
        .top(px(h.top + 18.0))
        .w(px(64.0))
        .gap(px(1.0))
        .px(design::ui_px(cx, 3.0))
        .py(design::ui_px(cx, 2.0))
        .rounded(design::ui_px(cx, 4.0))
        .bg(rgba_from(p.panel_high, 0.97))
        .border_1()
        .border_color(rgba_from(p.text_soft, 0.25));
    for (secs, label) in VOL_WINDOWS {
        let entity = entity.clone();
        let selected = u32::from(secs) == window_secs;
        menu = menu.child(
            div()
                .id(SharedString::from(format!("vol-win-{}-{}", h.pane, secs)))
                .px(design::ui_px(cx, 4.0))
                .py(px(1.0))
                .rounded(design::ui_px(cx, 3.0))
                .cursor_pointer()
                .when(selected, |s| s.bg(rgba_from(p.blue, 0.35)))
                .hover(|s| s.bg(rgba_from(p.blue, 0.5)))
                .child(mono(label.to_string(), p.text))
                .on_click(move |_, _w, app| {
                    entity.update(app, |this, cx| {
                        this.vol_menu_pane = None;
                        this.write_vol_cfg(cx, |c| c.window_secs = secs);
                    });
                }),
        );
    }

    div().child(row).child(menu).into_any_element()
}

impl ChartPanel {
    /// Mutate the global volume-view config, persist it, and repaint — the vol_view analogue of
    /// the arb popup's write path: the file is tiny and a change must reach every chart now.
    pub(super) fn write_vol_cfg(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut moon_core::config::VolViewCfg),
    ) {
        self.backend.update(cx, |b, bcx| {
            let before = b.vol_view.view;
            f(&mut b.vol_view.view);
            if b.vol_view.view != before {
                b.vol_view.save();
                bcx.notify();
            }
        });
        cx.notify();
    }

    /// Resolve one pane's header figures from the engine tape, the market's delta state, and the
    /// core's profit counters. `None` when the pane has no target yet.
    pub(super) fn vol_header_data(
        &self,
        b: &crate::Backend,
        idx: usize,
        core: CoreId,
        market: &str,
        window_secs: u32,
    ) -> VolHeader {
        let bvsv = self.chart.pane_volume_readout(idx, window_secs);
        let delta_24h = b
            .session
            .market_source()
            .market_ticker(core, market)
            .map(|t| t.delta_24h_pct)
            .filter(|d| d.is_finite());
        let ses = b
            .session
            .store()
            .core(core)
            .and_then(|d| d.profit)
            .map(|p| p.session_profit)
            .filter(|s| s.is_finite());
        VolHeader {
            pane: idx,
            bvsv,
            delta_24h,
            ses,
            left: 0.0,
            top: 0.0,
            right: 0.0,
        }
    }
}
