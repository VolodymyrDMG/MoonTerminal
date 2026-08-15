//! The "Арбитраж" popup: GLOBAL arbitrage overlay settings mirroring the bot's panel — master
//! switch, display flags (lines, legend numbers, absolute prices, spread percent, right-side
//! legend), and one row per known platform with an enable checkbox and a color swatch that
//! cycles a fixed palette. Persisted immediately to the portable `arb_view.toml`; every chart
//! (Main, Add tabs, detached windows) re-reads the config on its next render.
//!
//! The platform list is the union of what the cores are RELAYING right now and what the config
//! already names, so a platform enabled in the bot appears here as soon as its first price
//! arrives, and one the bot stopped relaying can still be unchecked or recolored.

use std::collections::BTreeSet;

use gpui::*;
use moon_core::config::ArbViewCfg;
use moon_ui::{
    h_flex, v_flex, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize,
    MoonPalette, MoonPopover, MoonPopoverPlacement, MoonToggle,
};
use rust_i18n::t;

use super::ChartTabs;
use crate::design;
use crate::panels::{popup_close_button, popup_group, popup_group_inset_px, popup_title};

/// Swatch palette the color button cycles through; starts at the platform's deterministic
/// default hue's nearest neighbor only by accident — the cycle order is fixed and predictable.
const SWATCHES: [[u8; 3]; 12] = [
    [230, 126, 34],
    [241, 196, 15],
    [46, 204, 113],
    [26, 188, 156],
    [52, 152, 219],
    [155, 89, 182],
    [233, 30, 99],
    [231, 76, 60],
    [149, 165, 166],
    [255, 255, 255],
    [121, 85, 72],
    [96, 125, 139],
];

/// Popup CONTENT width; two platform columns of ~150 plus the group inset.
pub(super) fn content_width(cx: &App) -> Pixels {
    px(2.0 * 156.0 + popup_group_inset_px(cx))
}

/// All platform names worth a row: currently relayed by any core, or already configured.
fn known_platforms(b: &crate::Backend, cfg: &ArbViewCfg) -> Vec<String> {
    let mut names: BTreeSet<String> = cfg.platforms.keys().cloned().collect();
    for s in b.session.sessions() {
        if let Some(core_st) = b.session.store().core(s.id) {
            for quotes in core_st.arb.values() {
                for q in quotes {
                    names.insert(q.platform_name.clone());
                }
            }
        }
    }
    names.into_iter().collect()
}

/// Mutate the global config through the backend, persisting and notifying on change.
fn write_cfg(entity: &Entity<ChartTabs>, app: &mut App, f: impl FnOnce(&mut ArbViewCfg)) {
    entity.update(app, |this, cx| {
        this.backend.update(cx, |b, bcx| {
            let before = b.arb_view.view.clone();
            f(&mut b.arb_view.view);
            if b.arb_view.view != before {
                b.arb_view.save();
                bcx.notify();
            }
        });
        cx.notify();
    });
}

/// Build one display-flag checkbox row.
fn flag_box(
    entity: &Entity<ChartTabs>,
    id: String,
    label: String,
    checked: bool,
    write: impl Fn(&mut ArbViewCfg, bool) + 'static,
) -> MoonCheckbox {
    let entity = entity.clone();
    MoonCheckbox::new(SharedString::from(id))
        .label(label)
        .checked(checked)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| {
            let v = *ch;
            write_cfg(&entity, app, |c| write(c, v));
        })
}

/// The gear-anchored popover host; mirrors `candle_popup_host`'s open/close shape.
pub(super) fn arb_popup_host(
    this: &ChartTabs,
    trigger: impl IntoElement,
    cx: &mut Context<ChartTabs>,
) -> MoonPopover {
    let open_entity = cx.entity();
    let popover = MoonPopover::new("chart-arb-popover")
        .placement(MoonPopoverPlacement::BottomEnd)
        .content_width(f32::from(content_width(cx)))
        .close_on_content_click(false)
        .open(this.arb_popup_open)
        .on_open_change(move |open, _window, app| {
            open_entity.update(app, |this, cx| {
                this.arb_popup_open = open;
                cx.notify();
            });
        })
        .trigger(trigger);
    if !this.arb_popup_open {
        return popover;
    }
    let p = MoonPalette::active(cx);
    let (cfg, platforms) = {
        let b = this.backend.read(cx);
        let cfg = b.arb_view.view.clone();
        let platforms = known_platforms(&b, &cfg);
        (cfg, platforms)
    };
    let entity = cx.entity();
    let close_entity = entity.clone();

    let head = h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .child(popup_title(t!("chart.arb.title").to_string(), p, cx))
        .child(popup_close_button("chart-arb-close", move |_, _w, app| {
            close_entity.update(app, |this, cx| {
                this.arb_popup_open = false;
                cx.notify();
            });
        }));

    let master = {
        let entity = entity.clone();
        MoonToggle::new("chart-arb-enabled")
            .checked(cfg.enabled)
            .label(t!("chart.arb.enabled").to_string())
            .on_change(move |v: &bool, _w, app| {
                let on = *v;
                write_cfg(&entity, app, |c| c.enabled = on);
            })
    };

    let flags = v_flex()
        .w_full()
        .gap(design::ui_px(cx, 4.0))
        .child(flag_box(
            &entity,
            "chart-arb-lines".into(),
            t!("chart.arb.lines").to_string(),
            cfg.lines,
            |c, v| c.lines = v,
        ))
        .child(flag_box(
            &entity,
            "chart-arb-numbers".into(),
            t!("chart.arb.numbers").to_string(),
            cfg.numbers,
            |c, v| c.numbers = v,
        ))
        .child(flag_box(
            &entity,
            "chart-arb-percent".into(),
            t!("chart.arb.percent").to_string(),
            cfg.percent,
            |c, v| c.percent = v,
        ))
        .child(flag_box(
            &entity,
            "chart-arb-prices".into(),
            t!("chart.arb.prices").to_string(),
            cfg.prices,
            |c, v| c.prices = v,
        ))
        .child(flag_box(
            &entity,
            "chart-arb-right".into(),
            t!("chart.arb.right").to_string(),
            cfg.right,
            |c, v| c.right = v,
        ));

    // Platform rows in two columns: color swatch (cycles the palette) + enable checkbox.
    let mut rows = v_flex().w_full().gap(design::ui_px(cx, 4.0));
    let mut it = platforms.into_iter().peekable();
    while it.peek().is_some() {
        let mut row = h_flex().w_full().gap(design::ui_px(cx, 8.0));
        for _ in 0..2 {
            let cell = if let Some(name) = it.next() {
                let pv = cfg.platform(&name);
                let color = design::rgb_to_u32(pv.color);
                let swatch = {
                    let entity = entity.clone();
                    let name = name.clone();
                    MoonButton::new(SharedString::from(format!("arb-color-{name}")))
                        .size(MoonButtonSize::Micro)
                        .variant(MoonButtonVariant::Ghost)
                        .text_segment("■", color, 700.0)
                        .tooltip(t!("chart.arb.color_tip").to_string())
                        .on_click(move |_, _w, app| {
                            let name = name.clone();
                            write_cfg(&entity, app, move |c| {
                                let mut pv = c.platform(&name);
                                let at = SWATCHES.iter().position(|s| *s == pv.color);
                                pv.color = SWATCHES[at.map_or(0, |i| (i + 1) % SWATCHES.len())];
                                c.set_platform(&name, pv);
                            });
                        })
                        .render()
                };
                let check = {
                    let entity = entity.clone();
                    let name = name.clone();
                    MoonCheckbox::new(SharedString::from(format!("arb-on-{name}")))
                        .label(name.clone())
                        .checked(pv.on)
                        .size(MoonCheckboxSize::Compact)
                        .on_change(move |ch: &bool, _w, app| {
                            let v = *ch;
                            let name = name.clone();
                            write_cfg(&entity, app, move |c| {
                                let mut pv = c.platform(&name);
                                pv.on = v;
                                c.set_platform(&name, pv);
                            });
                        })
                };
                h_flex()
                    .flex_1()
                    .items_center()
                    .gap(design::ui_px(cx, 2.0))
                    .child(swatch)
                    .child(check)
                    .into_any_element()
            } else {
                div().flex_1().into_any_element()
            };
            row = row.child(cell);
        }
        rows = rows.child(row);
    }
    let empty_hint = rows_is_empty(&cfg, this, cx).then(|| {
        div()
            .text_size(design::t_caption(cx))
            .text_color(rgb(p.text_muted))
            .child(t!("chart.arb.empty_hint").to_string())
    });

    popover.content(
        v_flex()
            .id("chart-arb-popup")
            .w_full()
            .gap(design::ui_px(cx, 8.0))
            .child(head)
            .child(master)
            .child(
                popup_group("chart-arb-flags", t!("chart.arb.frame_view").to_string()).child(flags),
            )
            .child(
                popup_group(
                    "chart-arb-platforms",
                    t!("chart.arb.frame_platforms").to_string(),
                )
                .child(rows)
                .children(empty_hint),
            ),
    )
}

/// Whether no platform is currently known — drawn as a hint instead of an empty frame.
fn rows_is_empty(cfg: &ArbViewCfg, this: &ChartTabs, cx: &App) -> bool {
    cfg.platforms.is_empty() && {
        let b = this.backend.read(cx);
        b.session.sessions().iter().all(|s| {
            b.session
                .store()
                .core(s.id)
                .is_none_or(|core_st| core_st.arb.is_empty())
        })
    }
}
