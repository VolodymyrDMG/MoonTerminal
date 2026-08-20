//! Arbitrage legend overlay: per-pane rows of other-exchange prices from the core's relay.
//!
//! Each row is a platform name plus the figures the arb-view flags ask for (spread percent,
//! absolute price), colored like that platform's chart line. A row is CLICKABLE when one of the
//! user's own cores in this workspace group trades that venue — the click resolves the same coin
//! on that core and opens it as a comparison tab, the terminal's "look at it over there" surface.
//! Platforms without a matching core (Forex, foreign venues) render as plain text: there is
//! nothing of the user's to open there.

use gpui::*;
use moon_ui::{rgba_from, v_flex, MoonPalette, MoonText};

use moon_core::config::ArbViewCfg;
use moon_core::session::CoreId;

use super::ChartPanel;
use crate::design;

/// One legend row, resolved at render time from the store and the arb-view config.
pub(super) struct ArbLegendRow {
    /// Bot-style platform name; doubles as the config key.
    pub name: String,
    /// Row color as 0xRRGGBB, shared with the platform's chart line.
    pub color: u32,
    /// Figures after the name: percent and/or absolute price per the config flags.
    pub text: String,
    /// Protocol platform code, carried into the click handler.
    pub platform: u8,
    /// Whether a same-group core trades this venue (regular venues only; HIP-3 deployer slots
    /// cannot be matched to a core yet).
    pub clickable: bool,
}

/// Resolve the legend rows for one pane's `(core, market)`.
///
/// Empty when the overlay or its numbers are disabled, the market has no relay data, or every
/// platform is unchecked. Rows keep the relay's platform order (the sweep's fixed table order),
/// which is stable across frames — no per-frame reshuffling.
pub(super) fn legend_rows(
    b: &crate::Backend,
    core: CoreId,
    market: &str,
    cfg: &ArbViewCfg,
    group: &str,
) -> Vec<ArbLegendRow> {
    if !cfg.enabled || !cfg.numbers {
        return Vec::new();
    }
    let Some(core_st) = b.session.store().core(core) else {
        return Vec::new();
    };
    let Some(quotes) = core_st.arb.get(market) else {
        return Vec::new();
    };
    // Venues tradable through the user's own cores IN THIS GROUP: platform byte → present.
    let venues = b.session.core_venues();
    let group_cores: Vec<CoreId> = b
        .session
        .sessions()
        .iter()
        .filter(|s| s.group == group)
        .map(|s| s.id)
        .collect();
    let mut rows = Vec::new();
    for q in quotes {
        let pv = cfg.platform(&q.platform_name);
        if !pv.on || !(q.price > 0.0) {
            continue;
        }
        let mut text = String::new();
        if cfg.prices {
            text.push_str(&moon_core::util::fmt::compact(f64::from(q.price), 6));
        }
        if cfg.percent {
            let pct = q
                .spread_pct()
                .and_then(|p| moon_core::util::fmt::signed_pct(f64::from(p), 2))
                .map(|(t, _)| t)
                .unwrap_or_else(|| "—".to_string());
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&pct);
        }
        // HIP-3 deployer slots (50..100) have no name channel in the relay, so no core match.
        let clickable = (q.platform < 50 || q.platform >= 100)
            && group_cores.iter().any(|id| {
                venues
                    .get(id)
                    .is_some_and(|v| v.id.code == q.platform && v.id.dex == 0)
            });
        rows.push(ArbLegendRow {
            name: q.platform_name.clone(),
            color: design::rgb_to_u32(pv.color),
            text,
            platform: q.platform,
            clickable,
        });
    }
    rows
}

/// Build the anchored legend block for one pane.
///
/// `left`/`right` are the pane's plot edges in logical pixels; the `right` config flag picks the
/// anchor side. Rows are compact monospace lines; clickable rows get a pointer cursor and a hover
/// backing so the "this is a button" affordance costs no extra chrome.
#[allow(clippy::too_many_arguments)]
pub(super) fn legend_element(
    entity: Entity<ChartPanel>,
    src: (CoreId, String),
    rows: Vec<ArbLegendRow>,
    left: f32,
    right: f32,
    top: f32,
    anchor_right: bool,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    // Doubled with the header readouts: the launch-size rows were unreadable at trading
    // distance (user report). The block widens with the font.
    const LEGEND_W: f32 = 210.0;
    let x = if anchor_right {
        (right - LEGEND_W - 4.0).max(left)
    } else {
        left
    };
    let mut col = v_flex()
        .absolute()
        .left(px(x))
        .top(px(top))
        .w(px(LEGEND_W))
        .gap(px(1.0))
        .px(design::ui_px(cx, 3.0))
        .py(design::ui_px(cx, 2.0))
        .rounded(design::ui_px(cx, 4.0))
        .bg(rgba_from(p.surface, 0.55));
    let fs = f32::from(design::ui_px(cx, 22.0));
    for (i, row) in rows.into_iter().enumerate() {
        let name_text = MoonText::new(row.name.clone())
            .color(row.color)
            .mono(true)
            .uppercase(false)
            .font_size(fs);
        let fig_text = MoonText::new(row.text.clone())
            .color(p.text_soft)
            .mono(true)
            .uppercase(false)
            .font_size(fs);
        let mut line = div()
            .id(SharedString::from(format!("arb-row-{}-{}", src.0, i)))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(design::ui_px(cx, 6.0))
            .px(px(1.0))
            .child(name_text.render())
            .child(fig_text.render());
        if row.clickable {
            let entity = entity.clone();
            let src = src.clone();
            let platform = row.platform;
            line = line
                .cursor_pointer()
                .hover(|s| s.bg(rgba_from(p.panel_high, 0.9)))
                .on_click(move |_, _w, app| {
                    entity.update(app, |this, cx| {
                        this.open_arb_platform(src.clone(), platform, cx);
                    });
                });
        }
        col = col.child(line);
    }
    col.into_any_element()
}

impl ChartPanel {
    /// Open the pane's coin on the user's core that trades `platform`, as a comparison tab.
    ///
    /// Resolution is lazy — at click time, not at render time — because it walks the target
    /// core's market catalog: same coin, preferring the same quote, then the shortest name (the
    /// plain pair before leveraged or dated variants). No match is a silent no-op: the legend
    /// offered the venue, but the target core's catalog does not carry the coin right now.
    pub(super) fn open_arb_platform(
        &mut self,
        src: (CoreId, String),
        platform: u8,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.workspace_group.clone() else {
            return;
        };
        let changed = self.backend.update(cx, |b, bcx| {
            let ms = b.session.market_source();
            let want = ms.market_label(src.0, &src.1);
            // The user's core on that venue, in this group.
            let venues = b.session.core_venues();
            let Some(target) = b
                .session
                .sessions()
                .iter()
                .filter(|s| s.group == group)
                .map(|s| s.id)
                .find(|id| {
                    venues
                        .get(id)
                        .is_some_and(|v| v.id.code == platform && v.id.dex == 0)
                })
            else {
                return false;
            };
            // Same coin on the target core; prefer the same quote, then the shortest symbol.
            let mut cands: Vec<(String, moon_core::market::MarketLabel)> = ms
                .search_markets(target, &want.coin, 24)
                .into_iter()
                .map(|m| {
                    let label = ms.market_label(target, &m);
                    (m, label)
                })
                .filter(|(_, l)| l.coin.eq_ignore_ascii_case(&want.coin))
                .collect();
            cands.sort_by_key(|(m, l)| {
                (
                    !l.quote.eq_ignore_ascii_case(&want.quote),
                    m.len(),
                    m.clone(),
                )
            });
            let Some((market, _)) = cands.into_iter().next() else {
                return false;
            };
            let authorized = b.open_single_tab_if_authorized(Some(&group), (target, market));
            if authorized {
                bcx.notify();
            }
            authorized
        });
        if changed {
            cx.notify();
        }
    }
}
