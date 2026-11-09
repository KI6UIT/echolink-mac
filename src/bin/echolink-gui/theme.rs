//! Look and feel: a bench-radio theme. Warm black chassis, amber phosphor,
//! signal green for receive, vermilion for transmit. Chakra Petch for labels,
//! DM Mono for anything that is data (callsigns, nodes, logs).

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Pos2, Rect, RichText, Stroke,
    StrokeKind, TextStyle, Vec2,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One complete colour scheme.
pub struct Palette {
    pub name: &'static str,
    pub dark: bool,
    pub chassis: Color32,
    pub panel: Color32,
    pub raised: Color32,
    pub hover: Color32,
    pub screen: Color32,
    pub edge: Color32,
    pub accent: Color32,
    pub accent_dim: Color32,
    pub on_accent: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub rx: Color32,
    pub tx: Color32,
    pub warn: Color32,
    pub scan_alpha: u8,
}

const fn c(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub const PALETTES: [Palette; 4] = [
    // Warm black chassis, amber phosphor.
    Palette {
        name: "Amber",
        dark: true,
        chassis: c(0x0f, 0x0d, 0x0a),
        panel: c(0x16, 0x13, 0x0e),
        raised: c(0x21, 0x1d, 0x15),
        hover: c(0x2e, 0x27, 0x17),
        screen: c(0x08, 0x07, 0x05),
        edge: c(0x3a, 0x33, 0x24),
        accent: c(0xff, 0xb0, 0x1f),
        accent_dim: c(0x9a, 0x6b, 0x14),
        on_accent: c(0x08, 0x07, 0x05),
        text: c(0xea, 0xe0, 0xc8),
        muted: c(0x8c, 0x82, 0x6b),
        rx: c(0x57, 0xe3, 0x89),
        tx: c(0xff, 0x4a, 0x2a),
        warn: c(0xff, 0xb0, 0x1f),
        scan_alpha: 55,
    },
    // Green-screen terminal.
    Palette {
        name: "Phosphor",
        dark: true,
        chassis: c(0x05, 0x0b, 0x07),
        panel: c(0x0a, 0x13, 0x0d),
        raised: c(0x11, 0x20, 0x17),
        hover: c(0x18, 0x32, 0x22),
        screen: c(0x02, 0x06, 0x03),
        edge: c(0x1f, 0x3d, 0x2b),
        accent: c(0x5c, 0xff, 0x9d),
        accent_dim: c(0x25, 0x9c, 0x58),
        on_accent: c(0x02, 0x06, 0x03),
        text: c(0xd2, 0xf4, 0xdc),
        muted: c(0x6a, 0x96, 0x7b),
        rx: c(0x5c, 0xff, 0x9d),
        tx: c(0xff, 0x5a, 0x4a),
        warn: c(0xe6, 0xd8, 0x4a),
        scan_alpha: 70,
    },
    // Night-watch navy with an ice-cyan display.
    Palette {
        name: "Cobalt",
        dark: true,
        chassis: c(0x06, 0x0a, 0x14),
        panel: c(0x0b, 0x11, 0x20),
        raised: c(0x14, 0x1d, 0x33),
        hover: c(0x1c, 0x2b, 0x4c),
        screen: c(0x03, 0x06, 0x0e),
        edge: c(0x27, 0x37, 0x5c),
        accent: c(0x4f, 0xd8, 0xff),
        accent_dim: c(0x2a, 0x84, 0xa6),
        on_accent: c(0x03, 0x06, 0x0e),
        text: c(0xdc, 0xe8, 0xf7),
        muted: c(0x73, 0x87, 0xa8),
        rx: c(0x57, 0xe3, 0x89),
        tx: c(0xff, 0x5a, 0x6e),
        warn: c(0xff, 0xc2, 0x3d),
        scan_alpha: 55,
    },
    // Daylight: logbook paper, fountain-pen ink.
    Palette {
        name: "Paper",
        dark: false,
        chassis: c(0xe6, 0xdd, 0xc6),
        panel: c(0xf3, 0xed, 0xdc),
        raised: c(0xfb, 0xf7, 0xea),
        hover: c(0xe4, 0xdb, 0xc2),
        screen: c(0xff, 0xfd, 0xf5),
        edge: c(0xc4, 0xb8, 0x98),
        accent: c(0x1d, 0x4e, 0x89),
        accent_dim: c(0x6a, 0x86, 0xad),
        on_accent: c(0xff, 0xfd, 0xf5),
        text: c(0x2b, 0x24, 0x18),
        muted: c(0x7a, 0x6f, 0x58),
        rx: c(0x1f, 0x8f, 0x4e),
        tx: c(0xd6, 0x34, 0x1a),
        warn: c(0xb9, 0x6a, 0x00),
        scan_alpha: 10,
    },
];

static CURRENT: AtomicUsize = AtomicUsize::new(0);

pub fn pal() -> &'static Palette {
    &PALETTES[CURRENT.load(Ordering::Relaxed).min(PALETTES.len() - 1)]
}
pub fn current() -> usize {
    CURRENT.load(Ordering::Relaxed)
}

fn pref_path() -> std::path::PathBuf {
    dirs::config_dir().unwrap_or_else(|| ".config".into()).join("echolink-tui/gui-theme")
}

/// The saved theme, by name (so reordering the list can't scramble it).
pub fn load_saved() -> usize {
    std::fs::read_to_string(pref_path())
        .ok()
        .and_then(|t| PALETTES.iter().position(|p| p.name.eq_ignore_ascii_case(t.trim())))
        .unwrap_or(0)
}

/// Switch theme, restyle the whole UI and remember the choice.
pub fn set(ctx: &egui::Context, idx: usize) {
    CURRENT.store(idx.min(PALETTES.len() - 1), Ordering::Relaxed);
    apply(ctx);
    let path = pref_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, pal().name);
}

pub fn chassis() -> Color32 {
    pal().chassis
}
pub fn panel() -> Color32 {
    pal().panel
}
pub fn raised() -> Color32 {
    pal().raised
}
pub fn screen() -> Color32 {
    pal().screen
}
pub fn edge() -> Color32 {
    pal().edge
}
pub fn accent() -> Color32 {
    pal().accent
}
pub fn accent_dim() -> Color32 {
    pal().accent_dim
}
pub fn on_accent() -> Color32 {
    pal().on_accent
}
pub fn text() -> Color32 {
    pal().text
}
pub fn muted() -> Color32 {
    pal().muted
}
pub fn rx() -> Color32 {
    pal().rx
}
pub fn tx() -> Color32 {
    pal().tx
}
pub fn warn() -> Color32 {
    pal().warn
}

pub fn display() -> FontFamily {
    FontFamily::Name("display".into())
}
pub fn mono_medium() -> FontFamily {
    FontFamily::Name("mono_medium".into())
}

/// Small spaced-out caps label, like silkscreen on a front panel.
pub fn silk(text: &str) -> RichText {
    RichText::new(text.to_uppercase()).font(FontId::new(11.0, display())).color(accent_dim())
}

/// Headline text in the display face.
pub fn head(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(FontId::new(size, display()))
}

/// Data text: callsigns, node numbers.
pub fn data(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(FontId::new(size, mono_medium()))
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    };
    add(&mut fonts, "chakra", include_bytes!("../../../assets/fonts/ChakraPetch-Regular.ttf"));
    add(&mut fonts, "chakra_semi", include_bytes!("../../../assets/fonts/ChakraPetch-SemiBold.ttf"));
    add(&mut fonts, "dmmono", include_bytes!("../../../assets/fonts/DMMono-Regular.ttf"));
    add(&mut fonts, "dmmono_med", include_bytes!("../../../assets/fonts/DMMono-Medium.ttf"));

    // Our faces first; egui's bundled fonts stay behind them for symbols and emoji.
    let fallback = |fonts: &FontDefinitions, f: FontFamily| fonts.families.get(&f).cloned().unwrap_or_default();
    let prop_tail = fallback(&fonts, FontFamily::Proportional);
    let mono_tail = fallback(&fonts, FontFamily::Monospace);
    let with = |head: &str, tail: &[String]| {
        let mut v = vec![head.to_string()];
        v.extend(tail.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, with("chakra", &prop_tail));
    fonts.families.insert(FontFamily::Monospace, with("dmmono", &mono_tail));
    fonts.families.insert(display(), with("chakra_semi", &prop_tail));
    fonts.families.insert(mono_medium(), with("dmmono_med", &mono_tail));
    ctx.set_fonts(fonts);
    CURRENT.store(load_saved(), Ordering::Relaxed);
    apply(ctx);
}

/// Push the current palette into egui's style.
fn apply(ctx: &egui::Context) {
    let k = pal();
    ctx.style_mut(|style| {
        use TextStyle::*;
        style.text_styles = [
            (Heading, FontId::new(22.0, display())),
            (Body, FontId::new(14.5, FontFamily::Proportional)),
            (Button, FontId::new(14.0, display())),
            (Small, FontId::new(12.0, FontFamily::Proportional)),
            (Monospace, FontId::new(13.5, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = Vec2::new(8.0, 7.0);
        style.spacing.button_padding = Vec2::new(11.0, 5.0);
        style.spacing.interact_size.y = 26.0;
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.floating = false;

        style.visuals = if k.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        let v = &mut style.visuals;
        v.override_text_color = Some(text());
        v.panel_fill = panel();
        v.window_fill = raised();
        v.window_stroke = Stroke::new(1.0, edge());
        v.window_corner_radius = CornerRadius::same(4);
        v.extreme_bg_color = screen();
        v.faint_bg_color = k.accent.gamma_multiply(0.03);
        v.hyperlink_color = accent();
        v.warn_fg_color = accent();
        v.error_fg_color = tx();
        v.selection.bg_fill = k.accent.gamma_multiply(0.18);
        v.selection.stroke = Stroke::new(1.0, accent());
        v.window_shadow = egui::epaint::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if k.dark { 160 } else { 50 }) };

        let r = CornerRadius::same(3);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = panel();
        w.noninteractive.bg_stroke = Stroke::new(1.0, edge());
        w.noninteractive.fg_stroke = Stroke::new(1.0, text());
        w.noninteractive.corner_radius = r;
        w.inactive.bg_fill = raised();
        w.inactive.weak_bg_fill = raised();
        w.inactive.bg_stroke = Stroke::new(1.0, edge());
        w.inactive.fg_stroke = Stroke::new(1.0, text());
        w.inactive.corner_radius = r;
        w.hovered.bg_fill = k.hover;
        w.hovered.weak_bg_fill = k.hover;
        w.hovered.bg_stroke = Stroke::new(1.0, accent_dim());
        w.hovered.fg_stroke = Stroke::new(1.5, accent());
        w.hovered.corner_radius = r;
        w.active.bg_fill = accent();
        w.active.weak_bg_fill = accent();
        w.active.bg_stroke = Stroke::new(1.0, accent());
        w.active.fg_stroke = Stroke::new(1.5, k.on_accent);
        w.active.corner_radius = r;
        w.open = w.inactive.clone();
    });
}

/// Fade-in progress (0..1) for a panel, started `delay` seconds after launch,
/// so the interface assembles itself in stages instead of appearing at once.
pub fn reveal(ctx: &egui::Context, id: &str, delay: f64) -> f32 {
    let t = ctx.input(|i| i.time) - delay;
    let on = t > 0.0;
    let v = ctx.animate_bool_with_time_and_easing(egui::Id::new(("reveal", id)), on, 0.45, egui::emath::easing::cubic_out);
    if v < 1.0 {
        ctx.request_repaint();
    }
    v
}

/// Faint horizontal scanlines, for the CRT/LCD feel on the readout.
pub fn scanlines(painter: &egui::Painter, rect: Rect) {
    let c = Color32::from_black_alpha(pal().scan_alpha);
    let mut y = rect.top() + 1.0;
    while y < rect.bottom() {
        painter.hline(rect.x_range(), y, Stroke::new(1.0, c));
        y += 3.0;
    }
}

/// A frequency-dial style ruler with a marker, painted along the bottom of a readout.
pub fn dial(ui: &mut egui::Ui, marker: Option<f32>, lit: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), egui::Sense::hover());
    let p = ui.painter();
    let n = ((rect.width() / 7.0) as usize).max(10);
    for i in 0..=n {
        let x = rect.left() + rect.width() * i as f32 / n as f32;
        let long = i % 5 == 0;
        let h = if long { 10.0 } else { 5.0 };
        let c = if long { accent_dim() } else { accent_dim().gamma_multiply(0.45) };
        p.vline(x, rect.bottom() - h..=rect.bottom(), Stroke::new(1.0, c));
    }
    if let Some(m) = marker {
        let x = rect.left() + rect.width() * m.clamp(0.0, 1.0);
        let c = if lit { accent() } else { accent_dim() };
        p.vline(x, rect.top()..=rect.bottom(), Stroke::new(2.0, c));
        p.add(egui::Shape::convex_polygon(
            vec![Pos2::new(x - 4.0, rect.top()), Pos2::new(x + 4.0, rect.top()), Pos2::new(x, rect.top() + 5.0)],
            c,
            Stroke::NONE,
        ));
    }
}

/// Segmented LED level meter: green, then amber, then red at the top end.
pub fn led_meter(ui: &mut egui::Ui, label: &str, level: f32, active: bool) {
    ui.horizontal(|ui| {
        ui.add_sized([26.0, 16.0], egui::Label::new(silk(label)));
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.0), egui::Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 2.0, screen());
        p.rect_stroke(rect, 2.0, Stroke::new(1.0, edge()), StrokeKind::Inside);
        let segs = 32usize;
        let gap = 2.0;
        let w = (rect.width() - 6.0 - gap * (segs as f32 - 1.0)) / segs as f32;
        let lit = (level.clamp(0.0, 1.0) * segs as f32).round() as usize;
        for i in 0..segs {
            let x = rect.left() + 3.0 + i as f32 * (w + gap);
            let seg = Rect::from_min_size(Pos2::new(x, rect.top() + 3.0), Vec2::new(w, rect.height() - 6.0));
            let frac = i as f32 / segs as f32;
            let on_color = if frac > 0.85 {
                tx()
            } else if frac > 0.65 {
                warn()
            } else {
                rx()
            };
            let color = if i < lit && active { on_color } else { on_color.gamma_multiply(0.13) };
            p.rect_filled(seg, 1.0, color);
        }
    });
}

/// A row of little swatches, one per theme. Returns the one clicked, if any.
pub fn picker(ui: &mut egui::Ui) -> Option<usize> {
    let mut chosen = None;
    // Right-to-left layouts place the first widget rightmost, so iterate backwards to read left to right.
    for (i, p) in PALETTES.iter().enumerate().rev() {
        let (rect, r) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::click());
        let painter = ui.painter();
        let ctr = rect.center();
        painter.circle_filled(ctr, 9.0, p.chassis);
        // Half the disc in the accent colour, like a split lamp lens.
        painter.add(egui::Shape::convex_polygon(
            vec![Pos2::new(ctr.x, ctr.y - 9.0), Pos2::new(ctr.x + 9.0, ctr.y), Pos2::new(ctr.x, ctr.y + 9.0)],
            p.accent,
            Stroke::NONE,
        ));
        painter.circle_filled(ctr, 5.5, p.chassis);
        painter.circle_filled(ctr, 3.5, p.accent);
        let selected = i == current();
        let ring = if selected {
            accent()
        } else if r.hovered() {
            muted()
        } else {
            edge()
        };
        painter.circle_stroke(ctr, 10.5, Stroke::new(if selected { 2.0 } else { 1.0 }, ring));
        if r.on_hover_text(format!("{} theme", p.name)).clicked() {
            chosen = Some(i);
        }
    }
    chosen
}
