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
    // Between the dim and full accent, so small labels stay readable on the dark panels.
    let c = accent_dim().lerp_to_gamma(accent(), 0.45);
    RichText::new(text.to_uppercase()).font(FontId::new(11.5, display())).color(c)
}

/// A clearly visible tab: filled in the accent colour when selected, outlined when not.
pub fn tab_button(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    let (fill, stroke, color) = if selected {
        (accent(), Stroke::new(1.0, accent()), on_accent())
    } else {
        (raised(), Stroke::new(1.0, accent_dim()), text())
    };
    ui.add(
        egui::Button::new(RichText::new(label).font(FontId::new(13.0, display())).color(color))
            .fill(fill)
            .stroke(stroke)
            .min_size(Vec2::new(130.0, 28.0)),
    )
    .clicked()
}

/// A section header that is a real button with a clear arrow, for things that open and close.
pub fn section_toggle(ui: &mut egui::Ui, label: &str, open: bool) -> bool {
    let r = ui.add(
        egui::Button::new(RichText::new(label).font(FontId::new(12.5, display())).color(text()))
            .fill(raised())
            .stroke(Stroke::new(1.0, accent_dim()))
            .min_size(Vec2::new(170.0, 26.0)),
    );
    // The arrow is painted, so it never depends on a font having the glyph.
    let c = Pos2::new(r.rect.right() - 14.0, r.rect.center().y);
    let pts = if open {
        vec![Pos2::new(c.x - 5.0, c.y - 3.0), Pos2::new(c.x + 5.0, c.y - 3.0), Pos2::new(c.x, c.y + 4.0)]
    } else {
        vec![Pos2::new(c.x - 3.0, c.y - 5.0), Pos2::new(c.x - 3.0, c.y + 5.0), Pos2::new(c.x + 4.0, c.y)]
    };
    ui.painter().add(egui::Shape::convex_polygon(pts, accent(), Stroke::NONE));
    r.clicked()
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
        w.inactive.bg_stroke = Stroke::new(1.0, muted().gamma_multiply(0.85)); // checkbox boxes and button edges
        w.inactive.fg_stroke = Stroke::new(1.6, text()); // and the tick inside them
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

/// Where a gauge's needle is, with its held peak.
#[derive(Clone, Copy, Default)]
struct Needle {
    pos: f32,
    peak: f32,
    peak_time: f64,
    clip_until: f64,
}

/// What a gauge measures: its coloured zones, scale marks and lamp.
#[derive(Clone, Copy, PartialEq)]
pub enum Scale {
    /// Signal level: green, yellow where speech should peak, red for clipping. Lamp: clip.
    Level,
    /// Gain reduction, 0 to 12 dB. Lamp: on while Auto level is running.
    Reduction,
    /// Noise removed, 0 to 30 dB. Lamp: on while noise reduction is running.
    Noise,
    /// Connection quality, 0 to 100, higher is better. Lamp: on while linked.
    Quality,
}

/// One gauge to draw.
pub struct Gauge<'a> {
    pub label: &'a str,
    /// 0 to 1 along the scale.
    pub value: f32,
    pub scale: Scale,
    /// The feature behind it is switched on; otherwise the needle rests and the face dims.
    pub active: bool,
}

/// A small panel of figures that sits among the gauges.
pub struct Card {
    pub title: String,
    pub title_color: Color32,
    pub lines: Vec<(String, Color32)>,
}

pub enum Cell<'a> {
    Gauge(Gauge<'a>),
    Card(Card),
}

/// What happened in the grid this frame.
#[derive(Default)]
pub struct GridEvent {
    /// A gauge was dragged from this position to that one.
    pub moved: Option<(usize, usize)>,
    /// "Reset gauge order" was chosen from a right-click menu.
    pub reset: bool,
}

/// Gauges and cards packed tightly in rows, as many across as fit. Drag one onto
/// another to rearrange; right-click to put them back.
pub fn gauge_grid(ui: &mut egui::Ui, cells: &[Cell]) -> GridEvent {
    let gap = 4.0;
    let avail = ui.available_width();
    let cols = (((avail + gap) / (165.0 + gap)).floor() as usize).clamp(2, cells.len().max(2));
    let w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).clamp(110.0, 200.0);
    // A meter face is about 1.8 wide to 1 tall.
    let h = (w / 1.8).clamp(68.0, 104.0);
    let size = Vec2::new(w, h);

    let drag_id = egui::Id::new("gauge-drag");
    let mut dragging: Option<usize> = ui.ctx().data(|d| d.get_temp(drag_id));
    let mut rects: Vec<Rect> = Vec::with_capacity(cells.len());
    let mut ev = GridEvent::default();

    ui.spacing_mut().item_spacing = Vec2::splat(gap);
    for (row_start, row) in cells.chunks(cols).enumerate().map(|(i, r)| (i * cols, r)) {
        ui.horizontal(|ui| {
            for (j, cell) in row.iter().enumerate() {
                let i = row_start + j;
                let resp = match cell {
                    Cell::Gauge(g) => vu_gauge(ui, g, size),
                    Cell::Card(c) => card(ui, c, size),
                };
                if resp.drag_started() {
                    dragging = Some(i);
                }
                if resp.hovered() && dragging.is_none() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
                resp.context_menu(|ui| {
                    if ui.button("Reset gauge order").clicked() {
                        ev.reset = true;
                        ui.close_menu();
                    }
                });
                rects.push(resp.rect);
            }
        });
    }

    if let Some(from) = dragging {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let pointer = ui.ctx().pointer_interact_pos();
        let target = pointer.and_then(|p| rects.iter().position(|r| r.contains(p)));
        if let Some(r) = rects.get(from) {
            ui.painter().rect_filled(*r, 6.0, Color32::from_black_alpha(90));
        }
        if let Some(t) = target.filter(|&t| t != from) {
            ui.painter().rect_stroke(rects[t].shrink(1.0), 6.0, Stroke::new(2.0, accent()), StrokeKind::Inside);
        }
        if ui.input(|i| i.pointer.any_released()) {
            if let Some(t) = target.filter(|&t| t != from) {
                ev.moved = Some((from, t));
            }
            dragging = None;
        }
    }
    ui.ctx().data_mut(|d| match dragging {
        Some(i) => d.insert_temp(drag_id, i),
        None => d.remove::<usize>(drag_id),
    });
    ev
}

fn card(ui: &mut egui::Ui, c: &Card, size: Vec2) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 6.0, screen());
    p.rect_stroke(rect, 6.0, Stroke::new(1.0, edge()), StrokeKind::Inside);
    let k = (rect.height() / 76.0).clamp(0.9, 1.45);
    p.text(Pos2::new(rect.left() + 10.0, rect.top() + 12.0 * k), egui::Align2::LEFT_CENTER, &c.title, FontId::new(13.0 * k, display()), c.title_color);
    for (i, (text, color)) in c.lines.iter().enumerate() {
        p.text(Pos2::new(rect.left() + 10.0, rect.top() + (29.0 + i as f32 * 14.0) * k), egui::Align2::LEFT_CENTER, text, FontId::new(11.0 * k, FontFamily::Monospace), *color);
    }
    resp
}

/// An old-school swing-needle gauge. The needle rises quickly and falls back slowly
/// like a real meter, a marker holds the recent peak, and a lamp shows clipping or
/// whether the feature is on.
pub fn vu_gauge(ui: &mut egui::Ui, g: &Gauge, size: Vec2) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let ctx = ui.ctx().clone();
    let id = egui::Id::new(("vu-gauge", g.label));
    let now = ctx.input(|i| i.time);
    let dt = ctx.input(|i| i.unstable_dt).clamp(0.001, 0.1);

    let mut n: Needle = ctx.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    let target = if g.active { g.value.clamp(0.0, 1.0) } else { 0.0 };
    let tau = if target > n.pos { 0.07 } else { 0.28 }; // quick up, slow down
    n.pos += (target - n.pos) * (1.0 - (-dt / tau).exp());
    if target >= n.peak {
        n.peak = target;
        n.peak_time = now;
    } else if now - n.peak_time > 1.2 {
        n.peak = (n.peak - 0.5 * dt).max(n.pos);
    }
    if g.scale == Scale::Level && target >= 0.98 {
        n.clip_until = now + 1.5;
    }
    ctx.data_mut(|d| d.insert_temp(id, n));
    if (n.pos - target).abs() > 0.002 || n.peak > n.pos + 0.002 {
        ctx.request_repaint();
    }

    let dim = if g.active { 1.0 } else { 0.35 };
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 6.0, screen());
    p.rect_stroke(rect, 6.0, Stroke::new(1.0, edge()), StrokeKind::Inside);

    // The needle pivots below the face; the face shows only the top of its swing.
    let k = (rect.height() / 76.0).clamp(0.9, 1.45); // everything on the face grows with it
    let span = 48f32.to_radians();
    let pivot = Pos2::new(rect.center().x, rect.bottom() + rect.height() * 0.20);
    let r = (rect.height() * 1.0).min(rect.width() * 0.64);
    let at = |v: f32, radius: f32| {
        let a = -span + 2.0 * span * v;
        pivot + Vec2::new(a.sin(), -a.cos()) * radius
    };
    let arc = |from: f32, to: f32, radius: f32, color: Color32, width: f32| {
        let steps = ((to - from) * 48.0).ceil().max(2.0) as usize;
        let pts: Vec<Pos2> = (0..=steps).map(|i| at(from + (to - from) * i as f32 / steps as f32, radius)).collect();
        p.add(egui::Shape::line(pts, Stroke::new(width, color.gamma_multiply(dim))));
    };

    // Coloured scale and its marks.
    let (zones, marks): (&[(f32, f32, Color32)], &[(f32, &str)]) = match g.scale {
        Scale::Level => (
            &[(0.0, 0.65, rx()), (0.65, 0.85, warn()), (0.85, 1.0, tx())],
            &[(0.1, "-20"), (0.25, "-12"), (0.5, "-6"), (0.7, "-3"), (1.0, "0")],
        ),
        Scale::Reduction => (
            &[(0.0, 0.5, rx()), (0.5, 0.75, warn()), (0.75, 1.0, tx())],
            &[(0.0, "0"), (0.25, "3"), (0.5, "6"), (0.75, "9"), (1.0, "12")],
        ),
        Scale::Noise => (&[(0.0, 1.0, rx())], &[(0.0, "0"), (0.33, "10"), (0.67, "20"), (1.0, "30")]),
        Scale::Quality => (
            &[(0.0, 0.4, tx()), (0.4, 0.7, warn()), (0.7, 1.0, rx())],
            &[(0.0, "0"), (0.25, "25"), (0.5, "50"), (0.75, "75"), (1.0, "100")],
        ),
    };
    for &(from, to, color) in zones {
        arc(from, to, r, color, 3.5 * k);
    }
    for i in 0..=20 {
        let v = i as f32 / 20.0;
        let len = (if i % 5 == 0 { 8.0 } else { 4.0 }) * k;
        p.line_segment([at(v, r - 3.0 * k), at(v, r - 3.0 * k - len)], Stroke::new(1.0, muted().gamma_multiply(dim)));
    }
    for &(v, text) in marks {
        p.text(at(v, r - 19.0 * k), egui::Align2::CENTER_CENTER, text, FontId::new(8.5 * k, FontFamily::Monospace), muted().gamma_multiply(dim));
    }
    let unit = match g.scale {
        Scale::Level => "",
        Scale::Reduction => " dB",
        Scale::Noise => " dB",
        Scale::Quality => "",
    };
    p.text(
        Pos2::new(rect.center().x, rect.bottom() - 9.0 * k),
        egui::Align2::CENTER_CENTER,
        if g.active { format!("{}{}", g.label, unit) } else { format!("{} OFF", g.label) },
        FontId::new(13.0 * k, display()),
        accent().gamma_multiply(dim.max(0.5)),
    );

    if g.active {
        if n.peak > 0.02 {
            p.line_segment([at(n.peak, r + 2.0 * k), at(n.peak, r + 7.0 * k)], Stroke::new(2.0, text().gamma_multiply(0.7)));
        }
        let tip = at(n.pos, r + 1.0);
        p.line_segment([pivot + Vec2::splat(1.5), tip + Vec2::splat(1.5)], Stroke::new(2.0 * k, Color32::from_black_alpha(80)));
        p.line_segment([pivot, tip], Stroke::new(1.6 * k, text()));
    }

    // Lamp: clipping for level gauges, "feature is on" for the others.
    let lamp = Pos2::new(rect.right() - 11.0 * k, rect.top() + 11.0 * k);
    let (lit, color) = match g.scale {
        Scale::Level => (now < n.clip_until, tx()),
        _ => (g.active, rx()),
    };
    if lit {
        p.circle_filled(lamp, 7.0 * k, color.gamma_multiply(0.25));
        if g.scale == Scale::Level {
            ctx.request_repaint();
        }
    }
    p.circle_filled(lamp, 3.5 * k, if lit { color } else { color.gamma_multiply(0.18) });
    p.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.center().y - 8.0)), 6.0, Color32::from_white_alpha(7));
    resp
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
