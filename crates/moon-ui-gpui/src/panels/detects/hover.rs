//! Hover popup for one detection card: the parameters of the detected situation next to an
//! enlarged frozen tick chart of the configured window (5/15/30 s) before the detection fired.
//!
//! The popup is a gpui tooltip built from [`HoverData`], a plain-data snapshot captured at
//! card-render time, so the builder closure owns no panel borrow and outlives the frame. The chart
//! reuses [`cards::ticks_canvas`] on the same shared rows the card mode draws, falling back to the
//! frozen five-minute candles and then to a "no trades" note, so the popup always explains itself.
//! Numbers follow the card idiom: header-delta colors for percentages, Moonbot-style short quote
//! amounts for the buy and sell turnover, and the shared adaptive price formatter.

use std::sync::Arc;

use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonPalette, MoonText, MoonTooltip, h_flex,
    rgba_from, v_flex,
};
use rust_i18n::t;

use moon_core::config::{BadgesConfig, DetectViewCfg};
use moon_core::market::DetectTick;

use super::{DetectItem, cards};
use crate::design;

/// Tooltip show delay in milliseconds: quicker than gpui's 500 ms default, because scanning a
/// detection feed is a hover-heavy workflow, yet long enough not to flash while crossing cards.
pub(super) const SHOW_DELAY_MS: u64 = 350;

/// Popup content width in logical pixels, sized for the enlarged chart to read per-trade detail.
const HOVER_W: f32 = 330.0;

/// Enlarged chart height in logical pixels; the strip fraction matches [`cards::ticks_canvas`].
const CHART_H: f32 = 160.0;

/// Frozen presentation snapshot for one card's hover popup.
///
/// Captured once per card render; strings and the theme are small clones while the tick rows ride
/// the card's shared [`Arc`]. Everything the popup prints is resolved here — the tooltip builder
/// runs later, when the panel borrow that produced it is long gone.
#[derive(Clone)]
pub(super) struct HoverData {
    /// Coin label without its quote suffix, the popup headline.
    base: String,
    /// Full market key, printed under the headline.
    market: String,
    core_name: String,
    /// Server color for the core badge, packed for [`MoonBadge`].
    core_color: u32,
    /// Human-readable strategy-kind name from the badge configuration, falling back to the code.
    kind_name: String,
    /// Badge code, color, and optional outline; `None` code when the badge is disabled.
    badge_code: Option<String>,
    badge_color: u32,
    badge_outline: Option<u32>,
    is_short: bool,
    /// Detection receipt time in Unix milliseconds, printed in the header clock's zone.
    born_ms: f64,
    zone: chrono_tz::Tz,
    /// Keep-alert lifetime of the card in whole seconds, from the strategy's `KeepAlert`.
    keep_secs: u32,
    /// Resolved exchange display name; empty when the core reported none.
    exchange: String,
    exchange_kind: String,
    delta_24h: f32,
    delta_1h: f32,
    /// Delta precision shared with the cards, from the gear popup.
    decimals: usize,
    /// Tick window in whole seconds from the gear popup; every tick-derived figure and label in
    /// the popup follows it.
    win_secs: u32,
    /// Frozen trades shared with the card's ticks chart mode (up to 30 s before the detection).
    ticks: Arc<Vec<DetectTick>>,
    /// Frozen five-minute candles for the chart fallback when no trades were retained.
    bars: Vec<(f32, f32, f32, f32)>,
    theme: moon_core::config::ChartTheme,
}

impl HoverData {
    /// Resolve one card's popup snapshot from the item and the feed's presentation config.
    pub(super) fn build(
        it: &DetectItem,
        cfg: &DetectViewCfg,
        theme: &moon_core::config::ChartTheme,
        badges: &BadgesConfig,
        is_light: bool,
        zone: chrono_tz::Tz,
    ) -> Self {
        // Mirror the card badge: code and colors only while the kind's badge is active. The kind
        // NAME is shown regardless — the popup exists to spell the parameters out.
        let badge_code = badges
            .active(it.kind)
            .then(|| badges.code(it.kind, it.is_short).to_string());
        let kind_name = badges
            .entry(it.kind)
            .map(|e| e.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| badges.code(it.kind, it.is_short).to_string());
        Self {
            base: it.base.clone(),
            market: it.market.clone(),
            core_name: it.core_name.clone(),
            core_color: design::rgb_to_u32(it.color),
            kind_name,
            badge_code,
            badge_color: design::rgb_to_u32(badges.color(it.kind, is_light)),
            badge_outline: badges
                .outline_color(it.kind, it.is_short, is_light)
                .map(design::rgb_to_u32),
            is_short: it.is_short,
            born_ms: it.born_ms,
            zone,
            keep_secs: (it.ttl_ms / 1000.0).round().max(0.0) as u32,
            exchange: crate::controls::exchange_display_name(&it.exchange_name),
            exchange_kind: it.exchange_kind.clone(),
            delta_24h: it.delta_24h,
            delta_1h: it.delta_1h,
            decimals: cfg.delta_decimals_clamped(),
            win_secs: cfg.ticks_window_secs_clamped(),
            ticks: Arc::clone(&it.ticks),
            bars: it.bars.clone(),
            theme: theme.clone(),
        }
    }
}

/// Aggregates of the windowed frozen trades printed in the popup's parameter grid.
#[derive(Debug, Default, PartialEq)]
pub(super) struct TickStats {
    pub trades: usize,
    /// Buy-side and sell-side quote turnover over the window.
    pub buy_quote: f32,
    pub sell_quote: f32,
    /// First-to-last price change in percent; `None` with fewer than two trades.
    pub win_pct: Option<f32>,
    /// Last windowed trade price — the market price at the detection moment.
    pub last_price: Option<f32>,
}

/// Fold rows (pre-trimmed to the configured window by [`cards::window_slice`]) into the popup's
/// aggregates.
///
/// Args:
///     ticks: Frozen windowed trades, oldest to newest.
///
/// Returns:
///     Turnover split by side, trade count, window price change, and the detection-moment price.
pub(super) fn tick_stats(ticks: &[DetectTick]) -> TickStats {
    let mut out = TickStats {
        trades: ticks.len(),
        last_price: ticks.last().map(|t| t.price),
        ..TickStats::default()
    };
    for t in ticks {
        if t.sell {
            out.sell_quote += t.quote;
        } else {
            out.buy_quote += t.quote;
        }
    }
    if let (Some(first), Some(last)) = (ticks.first(), ticks.last()) {
        if ticks.len() >= 2 && first.price > 0.0 {
            out.win_pct = Some((last.price / first.price - 1.0) * 100.0);
        }
    }
    out
}

/// High and low of the frozen rows for the chart's corner labels; `None` when empty.
pub(super) fn price_range(ticks: &[DetectTick]) -> Option<(f32, f32)> {
    let (mut hi, mut lo) = (f32::NEG_INFINITY, f32::INFINITY);
    for t in ticks {
        hi = hi.max(t.price);
        lo = lo.min(t.price);
    }
    (hi.is_finite() && lo.is_finite()).then_some((hi, lo))
}

/// Tooltip view: renders [`HoverData`] inside the shared [`MoonTooltip`] shell.
pub(super) struct DetectHoverView {
    data: HoverData,
}

impl DetectHoverView {
    pub(super) fn new(data: HoverData) -> Self {
        Self { data }
    }
}

impl Render for DetectHoverView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let d = &self.data;
        // Every tick-derived figure follows the gear popup's window, matching the card chart.
        let win_ms = d.win_secs as f32 * 1000.0;
        let win = cards::window_slice(&d.ticks, win_ms);
        let stats = tick_stats(win);
        let (pos, neg) = (design::positive_color(p), design::danger_color(p));

        // --- Header: coin, card badge, direction, and detection wall-clock time. ---
        let time = moon_core::util::display_time::at_millis(d.born_ms as i64, d.zone)
            .map(|v| v.format("%H:%M:%S").to_string())
            .unwrap_or_default();
        let mut head = h_flex().w_full().items_center().gap(px(6.0)).child(
            MoonText::new(d.base.clone())
                .color(p.text)
                .font_size(13.0)
                .line_height(16.0)
                .weight(700.0)
                .mono(true)
                .uppercase(false)
                .render(),
        );
        if let Some(code) = &d.badge_code {
            let mut badge = MoonBadge::new(code.clone())
                .variant(MoonBadgeVariant::Soft)
                .size(MoonBadgeSize::Tiny)
                .bg_color(d.badge_color)
                .text_color(d.badge_color)
                .mono(true);
            if let Some(oc) = d.badge_outline {
                badge = badge.border_color(oc).border_alpha(0.9);
            }
            head = head.child(badge.render());
        }
        head = head
            .child(
                MoonText::new(if d.is_short { "Short" } else { "Long" })
                    .color(if d.is_short { neg } else { pos })
                    .weight(700.0)
                    .mono(true)
                    .uppercase(false)
                    .render(),
            )
            .child(div().flex_1())
            .child(mut_text(time, p).render());

        // --- Identity: market, exchange, and strategy kind with the core badge. ---
        let mut ident = h_flex()
            .w_full()
            .items_center()
            .gap(px(6.0))
            .child(soft_text(d.market.clone(), p).render());
        if !d.exchange.is_empty() {
            ident = ident.child(soft_text(d.exchange.clone(), p).render());
        }
        if !d.exchange_kind.is_empty() {
            ident = ident.child(mut_text(d.exchange_kind.clone(), p).render());
        }
        let strategy = h_flex()
            .w_full()
            .items_center()
            .gap(px(6.0))
            .child(mut_text(d.kind_name.clone(), p).render())
            .child(div().flex_1())
            .child(
                MoonBadge::new(d.core_name.clone())
                    .variant(MoonBadgeVariant::Soft)
                    .size(MoonBadgeSize::Tiny)
                    .bg_color(d.core_color)
                    .text_color(d.core_color)
                    .border_color(d.core_color)
                    .border_alpha(0.4)
                    .mono(true)
                    .render(),
            );

        // --- Enlarged frozen chart with price-range corner labels. ---
        let chart_h = design::ui_value(cx, CHART_H);
        let (chart, is_ticks) = match cards::ticks_canvas(&d.ticks, &d.theme, win_ms) {
            Some(c) => (Some(c), true),
            // Same fallback order as the card mode: candles, then an explanatory note.
            None => (cards::candle_canvas(&d.bars, &d.theme), false),
        };
        let chart_block = if let Some(c) = chart {
            let mut z = div()
                .relative()
                .w_full()
                .h(px(chart_h))
                .flex_none()
                .rounded(design::ui_px(cx, 4.0))
                .border_1()
                .border_color(rgba_from(p.border, 0.7))
                .bg(rgba_from(p.surface, 0.5))
                .overflow_hidden()
                .child(c);
            if is_ticks {
                if let Some((hi, lo)) = price_range(win) {
                    // The low label clears the volume strip, whose height rule the canvas shares.
                    let strip_h = (chart_h * 0.26).clamp(3.0, 40.0);
                    z =
                        z.child(div().absolute().top(px(2.0)).right(px(4.0)).child(
                            mut_text(crate::panels::common::num(f64::from(hi)), p).render(),
                        ))
                        .child(
                            div()
                                .absolute()
                                .bottom(px(strip_h + 3.0))
                                .right(px(4.0))
                                .child(
                                    mut_text(crate::panels::common::num(f64::from(lo)), p).render(),
                                ),
                        );
                }
            }
            z.into_any_element()
        } else {
            div()
                .w_full()
                .h(px(design::ui_value(cx, 34.0)))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    mut_text(t!("detects.hover.no_ticks", s = d.win_secs).to_string(), p).render(),
                )
                .into_any_element()
        };

        // --- Parameter grid: two labelled cells per row, card-style delta colors. ---
        let pct = |v: f32| pct_text(v, d.decimals, p);
        let row1 = grid_row(
            cell(t!("detects.field.d24").to_string(), pct(d.delta_24h), p),
            cell(t!("detects.field.d1").to_string(), pct(d.delta_1h), p),
        );
        let dwin = match stats.win_pct {
            Some(v) => pct_text(v, d.decimals, p),
            None => mut_text("—".to_string(), p).render().into_any_element(),
        };
        let price = match stats.last_price {
            Some(v) => strong_text(crate::panels::common::num(f64::from(v)), p.text_soft)
                .render()
                .into_any_element(),
            None => mut_text("—".to_string(), p).render().into_any_element(),
        };
        let row2 = grid_row(
            cell(t!("detects.hover.d30", s = d.win_secs).to_string(), dwin, p),
            cell(t!("detects.hover.price").to_string(), price, p),
        );
        let quote = |v: f32, col: u32| {
            strong_text(crate::chartdx::volume_graph::format_quote_short(v), col)
                .render()
                .into_any_element()
        };
        let row3 = grid_row(
            cell(
                t!("detects.hover.buys", s = d.win_secs).to_string(),
                quote(stats.buy_quote, pos),
                p,
            ),
            cell(
                t!("detects.hover.sells", s = d.win_secs).to_string(),
                quote(stats.sell_quote, neg),
                p,
            ),
        );
        let row4 = grid_row(
            cell(
                t!("detects.hover.trades", s = d.win_secs).to_string(),
                soft_text(stats.trades.to_string(), p)
                    .render()
                    .into_any_element(),
                p,
            ),
            cell(
                t!("detects.hover.keep").to_string(),
                soft_text(format!("{}s", d.keep_secs), p)
                    .render()
                    .into_any_element(),
                p,
            ),
        );

        let hint = mut_text(t!("detects.hover.hint").to_string(), p).render();

        MoonTooltip::empty("detect-hover")
            .width(design::ui_value(cx, HOVER_W))
            .max_width(design::ui_value(cx, HOVER_W))
            .child(
                v_flex()
                    .w_full()
                    .gap(px(4.0))
                    .child(head)
                    .child(ident)
                    .child(strategy)
                    .child(chart_block)
                    .child(
                        v_flex()
                            .w_full()
                            .gap(px(2.0))
                            .child(row1)
                            .child(row2)
                            .child(row3)
                            .child(row4),
                    )
                    .child(hint),
            )
    }
}

/// Small muted monospace label, the popup's secondary text tone.
fn mut_text(text: String, p: MoonPalette) -> MoonText {
    MoonText::new(text)
        .color(p.text_muted)
        .mono(true)
        .uppercase(false)
}

/// Soft-tone monospace label for identity values.
fn soft_text(text: String, p: MoonPalette) -> MoonText {
    MoonText::new(text)
        .color(p.text_soft)
        .mono(true)
        .uppercase(false)
}

/// Bold monospace value in an explicit color.
fn strong_text(text: String, color: u32) -> MoonText {
    MoonText::new(text)
        .color(color)
        .weight(700.0)
        .mono(true)
        .uppercase(false)
}

/// Percentage value with the header-delta color contract: classified by the ROUNDED value.
fn pct_text(v: f32, decimals: usize, p: MoonPalette) -> AnyElement {
    let (label, col) = match moon_core::util::fmt::signed_pct(f64::from(v), decimals) {
        Some((text, sign)) => (
            text,
            sign.pick(
                design::positive_color(p),
                design::danger_color(p),
                p.text_soft,
            ),
        ),
        None => ("—".to_string(), p.text_muted),
    };
    strong_text(label, col).render().into_any_element()
}

/// One labelled grid cell: muted label left, value right.
fn cell(label: String, value: AnyElement, p: MoonPalette) -> Div {
    h_flex()
        .flex_1()
        .min_w(px(0.0))
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .child(mut_text(label, p).render())
        .child(value)
}

/// One grid row of two cells with the popup's column gap.
fn grid_row(a: Div, b: Div) -> Div {
    h_flex()
        .w_full()
        .items_center()
        .gap(px(10.0))
        .child(a)
        .child(b)
}
