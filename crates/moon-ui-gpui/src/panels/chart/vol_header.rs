//! Chart header readouts mirrored from the bot's chart caption: the Bv/Sv block (buy/sell quote
//! volume over a click-configurable trailing window, with net delta and a pressure ratio bar),
//! the coin's 24-hour delta, and the core's session profit ("Ses").
//!
//! Placement is a small CONSTRUCTOR, because one fixed spot proved to collide with the canvas
//! caption plates: each element carries a slot from `vol_view.toml` — two bands (plot top, plot
//! bottom just above the volume zone) × three anchors, or hidden. Elements sharing a slot line
//! up in a row instead of stacking on each other. The window chip opens a small menu anchored to
//! itself; the choice is global and feeds the same tape the volume band draws, so the figures
//! and the graph can never disagree about what a "buy" was.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, MoonText, rgba_from, v_flex};

use moon_core::config::{self, VolViewCfg};
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

/// One band's geometry in logical pixels: left edge, right edge, and the row's top.
#[derive(Clone, Copy)]
pub(super) struct BandRect {
    pub left: f32,
    pub right: f32,
    pub top: f32,
}

/// Resolved figures and geometry for one pane's header overlays.
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
    /// Top band: along the plot's top edge, clear of the corner pin/lock and close controls.
    pub top: BandRect,
    /// Bottom band: just above the volume zone (or above the time axis when the zone is off).
    pub bottom: BandRect,
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

fn mono(text: String, color: u32) -> AnyElement {
    MoonText::new(text)
        .color(color)
        .mono(true)
        .uppercase(false)
        .render()
        .into_any_element()
}

/// The Bv/Sv block: window chip (with its anchored menu), per-side figures, net delta, and the
/// buy/sell pressure bar.
fn vol_block(
    entity: Entity<ChartPanel>,
    pane: usize,
    bvsv: Option<(f32, f32)>,
    window_secs: u32,
    menu_open: bool,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let mut row = div()
        .id(SharedString::from(format!("vol-block-{pane}")))
        .flex()
        .flex_row()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .px(design::ui_px(cx, 4.0))
        .py(px(1.0))
        .rounded(design::ui_px(cx, 3.0))
        .bg(rgba_from(p.surface, 0.4));

    // Window chip: the interactive piece — its menu anchors to the chip itself, so the block
    // can live in any slot without the menu drifting.
    {
        let toggle = entity.clone();
        let mut chip = div()
            .id(SharedString::from(format!("vol-win-chip-{pane}")))
            .relative()
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
                toggle.update(app, |this, cx| {
                    this.vol_menu_pane = if this.vol_menu_pane == Some(pane) {
                        None
                    } else {
                        Some(pane)
                    };
                    cx.notify();
                });
            });
        if menu_open {
            let mut menu = v_flex()
                .absolute()
                .left(px(0.0))
                .top(px(17.0))
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
                        .id(SharedString::from(format!("vol-win-{pane}-{secs}")))
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
            chip = chip.child(menu);
        }
        row = row.child(chip);
    }

    if let Some((bv, sv)) = bvsv {
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
    row.into_any_element()
}

/// A plain figure chip on the shared translucent plate.
fn chip(id: String, text: String, color: u32, p: MoonPalette, cx: &App) -> AnyElement {
    div()
        .id(SharedString::from(id))
        .px(design::ui_px(cx, 4.0))
        .py(px(1.0))
        .rounded(design::ui_px(cx, 3.0))
        .bg(rgba_from(p.surface, 0.4))
        .child(mono(text, color))
        .into_any_element()
}

/// Build the overlay elements for one pane: up to two band containers, each a full-width row
/// with left/center/right groups, so elements sharing a slot sit side by side instead of on top
/// of each other.
pub(super) fn header_overlays(
    entity: Entity<ChartPanel>,
    h: VolHeader,
    cfg: VolViewCfg,
    menu_open: bool,
    p: MoonPalette,
    cx: &App,
) -> Vec<AnyElement> {
    // Fixed assembly order inside a slot: the Bv/Sv block, then 24h, then Ses.
    let mut slots: [Vec<AnyElement>; 6] = Default::default();
    let mut place = |pos: u8, el: AnyElement| {
        let pos = VolViewCfg::pos_clamped(pos);
        if pos != config::VOL_POS_HIDDEN {
            slots[usize::from(pos)].push(el);
        }
    };
    place(
        cfg.pos_vol,
        vol_block(
            entity.clone(),
            h.pane,
            h.bvsv,
            cfg.window_secs_clamped(),
            menu_open,
            p,
            cx,
        ),
    );
    if let Some(d) = h.delta_24h {
        place(
            cfg.pos_delta,
            chip(
                format!("vol-24h-{}", h.pane),
                format!("24h {d:+.1}%"),
                signed_color(&p, d),
                p,
                cx,
            ),
        );
    }
    if let Some(ses) = h.ses {
        place(
            cfg.pos_ses,
            chip(
                format!("vol-ses-{}", h.pane),
                format!("Ses {ses:+.2}$"),
                signed_color(&p, ses),
                p,
                cx,
            ),
        );
    }

    let mut out = Vec::new();
    let bands = [(0usize, h.top), (3usize, h.bottom)];
    for (base, rect) in bands {
        let [l, c, r] = {
            let mut it = slots[base..base + 3].iter_mut();
            [
                std::mem::take(it.next().expect("slot")),
                std::mem::take(it.next().expect("slot")),
                std::mem::take(it.next().expect("slot")),
            ]
        };
        if l.is_empty() && c.is_empty() && r.is_empty() {
            continue;
        }
        let group = |children: Vec<AnyElement>, justify: fn(Div) -> Div| {
            justify(div().flex_1().flex().flex_row().items_center())
                .gap(design::ui_px(cx, 8.0))
                .children(children)
        };
        out.push(
            div()
                .id(SharedString::from(format!(
                    "vol-band-{}-{}",
                    h.pane,
                    if base == 0 { "top" } else { "bottom" }
                )))
                .absolute()
                .left(px(rect.left))
                .top(px(rect.top))
                .w(px((rect.right - rect.left).max(60.0)))
                .flex()
                .flex_row()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(group(l, |d| d.justify_start()))
                .child(group(c, |d| d.justify_center()))
                .child(group(r, |d| d.justify_end()))
                .into_any_element(),
        );
    }
    out
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
    /// core's profit counters. Band geometry is filled by the caller, which owns the pane rects.
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
        let empty = BandRect {
            left: 0.0,
            right: 0.0,
            top: 0.0,
        };
        VolHeader {
            pane: idx,
            bvsv,
            delta_24h,
            ses,
            top: empty,
            bottom: empty,
        }
    }
}
