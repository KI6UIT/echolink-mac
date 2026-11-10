//! EchoLink desktop app: the same station core as echolink-tui, with a
//! window, a big push-to-talk button and a settings dialog.

mod settings;
mod theme;

use eframe::egui::{self, Color32, FontFamily, RichText};
use echolink::config::{self, Config};
use echolink::directory::Station;
use echolink::qso::{State, TX_LIMIT};
use echolink::session::{Filter, LogKind, Session, Update};
use egui_extras::{Column, TableBuilder};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const ECHOTEST_NODE: u32 = 9999;
const RESTART_ENV: &str = "ECHOLINK_GUI_RESTARTED";

fn main() -> eframe::Result {
    echolink::log::init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("EchoLink")
            .with_app_id("echolink-gui")
            .with_inner_size([1150.0, 720.0])
            .with_min_inner_size([800.0, 520.0]),
        // Don't block on vsync: a window on a hidden workspace gets no frame
        // callbacks, which would otherwise stall the whole UI thread.
        vsync: false,
        ..Default::default()
    };
    eframe::run_native("EchoLink", options, Box::new(|cc| Ok(Box::new(Gui::new(cc)))))
}

struct Gui {
    session: Option<Session>,
    start_error: Option<String>,
    settings: Option<settings::Form>,
    filter: Filter,
    search: String,
    favorites_only: bool,
    visible: Vec<usize>,
    selected: Option<String>,
    node_input: String,
    chat_input: String,
    ptt_held_by_mouse: bool,
    ptt_on: bool,
    closing: bool,
    /// Whether the window has focus; notifications only pop up when it doesn't.
    focused: Arc<AtomicBool>,
    /// Mic clip lamp: last seen clip count, and when the lamp goes out.
    last_clips: u32,
    clip_until: Option<Instant>,
    /// Result of the last auto-set, shown under the mic controls.
    mic_note: String,
}

impl Gui {
    fn new(cc: &eframe::CreationContext) -> Gui {
        theme::install(&cc.egui_ctx);
        let mut gui = Gui {
            session: None,
            start_error: None,
            settings: None,
            filter: Filter::All,
            search: String::new(),
            favorites_only: false,
            visible: Vec::new(),
            selected: None,
            node_input: String::new(),
            chat_input: String::new(),
            ptt_held_by_mouse: false,
            ptt_on: false,
            closing: false,
            focused: Arc::new(AtomicBool::new(true)),
            last_clips: 0,
            clip_until: None,
            mic_note: String::new(),
        };
        match config::load() {
            Ok(cfg) => {
                if std::env::var_os(RESTART_ENV).is_some() {
                    // Give the previous instance a moment to release its ports.
                    std::thread::sleep(Duration::from_millis(1500));
                }
                gui.start(cfg);
            }
            Err(_) => gui.settings = Some(settings::Form::new(None)),
        }
        gui
    }

    fn start(&mut self, cfg: Config) {
        let focused = self.focused.clone();
        let notifier: echolink::session::Notifier = Box::new(move |text| {
            if !focused.load(Ordering::Relaxed) {
                notify(text);
            }
        });
        match Session::start(cfg, "echolink-gui", Some(notifier)) {
            Ok(s) => {
                self.session = Some(s);
                self.start_error = None;
            }
            Err(e) => self.start_error = Some(format!("{e:#}")),
        }
    }

    fn favorites(&self) -> &[String] {
        self.session.as_ref().map(|s| s.cfg.favorites.as_slice()).unwrap_or(&[])
    }

    fn refilter(&mut self) {
        let Some(s) = &self.session else { return };
        let favs = &s.cfg.favorites;
        let mut v = s.matching(self.filter, &self.search);
        if self.favorites_only {
            v.retain(|&i| favs.iter().any(|f| f == &s.stations[i].callsign));
        }
        // Favorites first, then alphabetical (the list is already sorted).
        v.sort_by_key(|&i| !favs.iter().any(|f| f == &s.stations[i].callsign));
        self.visible = v;
    }

    /// Save the mic settings onto whatever is on disk.
    fn save_mic_settings(&mut self) {
        let Some(s) = &mut self.session else { return };
        s.cfg.mic_gain_db = s.audio.shared.gain_db();
        s.cfg.auto_gain = s.audio.shared.auto_gain.load(Ordering::Relaxed);
        s.cfg.noise_reduction = s.audio.shared.denoise.load(Ordering::Relaxed);
        if let Ok(mut on_disk) = config::load() {
            on_disk.mic_gain_db = s.cfg.mic_gain_db;
            on_disk.auto_gain = s.cfg.auto_gain;
            on_disk.noise_reduction = s.cfg.noise_reduction;
            if let Err(e) = config::save(&on_disk) {
                s.push(LogKind::System, format!("Couldn't save mic settings: {e:#}"));
            }
        }
    }

    /// RX and MIC level meters, mic gain, auto-set and auto-level.
    /// Aim: speech peaks in the yellow (heard), never the red (clipping).
    fn audio_panel(&mut self, ui: &mut egui::Ui) {
        use echolink::audio::{CalState, GAIN_MAX_DB, GAIN_MIN_DB};
        let Some(s) = &self.session else { return };
        let shared = s.audio.shared.clone();
        let rx = shared.rx_level.load(Ordering::Relaxed) as f32 / 1000.0;
        let mic = shared.mic_level.load(Ordering::Relaxed) as f32 / 1000.0;
        theme::led_meter(ui, "RX", rx, true);
        theme::led_meter(ui, "MIC", mic, true);

        let clips = shared.clips.load(Ordering::Relaxed);
        if clips != self.last_clips {
            self.last_clips = clips;
            self.clip_until = Some(Instant::now() + Duration::from_millis(1500));
        }
        let clipping = self.clip_until.is_some_and(|t| Instant::now() < t);

        let mut save = false;
        if let Some(result) = shared.take_calibration_result() {
            self.mic_note = match result {
                CalState::Done(db) => format!("Mic gain set to {db:+.1} dB. Your voice should now reach the yellow."),
                _ => "Couldn't hear the microphone. Check that it isn't muted and that the right input is selected in System Settings, then try again.".into(),
            };
            save = matches!(result, CalState::Done(_));
        }
        let cal = shared.calibration();

        ui.horizontal(|ui| {
            ui.add_sized([26.0, 18.0], egui::Label::new(theme::silk("GAIN")));
            let mut db = shared.gain_db();
            let r = ui.add_enabled(
                !matches!(cal, CalState::Listening(_)),
                egui::Slider::new(&mut db, GAIN_MIN_DB..=GAIN_MAX_DB).suffix(" dB").step_by(0.5).fixed_decimals(1),
            );
            if r.changed() {
                shared.set_gain_db(db);
                self.mic_note.clear();
            }
            save |= r.drag_stopped() || (r.changed() && !r.dragged());
        });
        ui.horizontal(|ui| {
            match cal {
                CalState::Listening(p) => {
                    if ui.button("CANCEL").clicked() {
                        shared.cancel_calibration();
                    }
                    ui.label(
                        RichText::new(format!(
                            "Listening… keep talking at your normal level ({:.0} s)",
                            (1.0 - p) * echolink::audio::CAL_SECONDS
                        ))
                        .color(theme::accent()),
                    );
                }
                _ => {
                    if ui
                        .button("AUTO-SET")
                        .on_hover_text("Talk normally for 5 seconds and the gain is set so your voice peaks in the yellow without touching the red")
                        .clicked()
                    {
                        self.mic_note.clear();
                        shared.start_calibration();
                    }
                    let mut auto = shared.auto_gain.load(Ordering::Relaxed);
                    if ui
                        .checkbox(&mut auto, "Auto level")
                        .on_hover_text("Keeps adjusting while you talk: lifts quiet speech, pulls loud speech down, and never lets a peak reach the red")
                        .changed()
                    {
                        shared.auto_gain.store(auto, Ordering::Relaxed);
                        save = true;
                    }
                    let mut quiet = shared.denoise.load(Ordering::Relaxed);
                    if ui
                        .checkbox(&mut quiet, "Noise reduction")
                        .on_hover_text("Removes background noise from the room (fans, keyboard, hum) using the open-source RNNoise filter")
                        .changed()
                    {
                        shared.denoise.store(quiet, Ordering::Relaxed);
                        save = true;
                    }
                    let (text, color) = if clipping { ("CLIP", theme::tx()) } else { ("clip", theme::muted()) };
                    ui.label(RichText::new(text).monospace().color(color))
                        .on_hover_text("Lights when the mic signal hits full scale. Turn the gain down.");
                }
            }
        });
        let note = if !self.mic_note.is_empty() {
            self.mic_note.as_str()
        } else if clipping {
            "Too loud: the red is lit. Lower the gain, or use Auto level."
        } else {
            "Talk normally: the MIC bar should reach the yellow, not the red."
        };
        ui.label(RichText::new(note).small().color(theme::muted()));
        if save {
            self.save_mic_settings();
        }
    }

    fn toggle_favorite(&mut self, call: &str) {
        let Some(s) = &mut self.session else { return };
        let favs = &mut s.cfg.favorites;
        match favs.iter().position(|f| f == call) {
            Some(i) => drop(favs.remove(i)),
            None => favs.push(call.to_string()),
        }
        // Save just the favorites onto whatever is on disk.
        if let Ok(mut on_disk) = config::load() {
            on_disk.favorites = s.cfg.favorites.clone();
            if let Err(e) = config::save(&on_disk) {
                s.push(LogKind::System, format!("Couldn't save favorites: {e:#}"));
            }
        }
        self.refilter();
    }

    /// Disconnect and log off, waiting (briefly) for it to finish so it
    /// doesn't depend on the window drawing another frame.
    fn shutdown(&mut self) {
        let Some(s) = &mut self.session else { return };
        echolink::log!("closing: disconnecting and logging off");
        s.quit();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if s.poll() == Update::Finished {
                echolink::log!("closing: logged off");
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        echolink::log!("closing: log-off timed out");
    }

    fn selected_station(&self) -> Option<&Station> {
        let s = self.session.as_ref()?;
        s.find(self.selected.as_deref()?)
    }
}

#[cfg(not(target_os = "macos"))]
fn notify(text: &str) {
    let _ = std::process::Command::new("notify-send")
        .args(["--app-name=EchoLink", "--icon=audio-input-microphone", "EchoLink", text])
        .spawn();
}

#[cfg(target_os = "macos")]
fn notify(text: &str) {
    // Pass the text as an argument so quotes in chat can't break out of the script.
    let _ = std::process::Command::new("osascript")
        .args([
            "-e",
            "on run argv\ndisplay notification (item 1 of argv) with title \"EchoLink\"\nend run",
            text,
        ])
        .spawn();
}

impl eframe::App for Gui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(50));

        // Background events.
        if let Some(s) = &mut self.session
            && s.poll() == Update::StationsChanged
        {
            self.refilter();
        }
        self.focused.store(ctx.input(|i| i.focused), Ordering::Relaxed);

        // Closing the window disconnects and logs off first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.closing {
            self.closing = true;
            self.shutdown();
        }

        // Settings dialog (also the first-run screen).
        if let Some(form) = &mut self.settings {
            match form.show(ctx) {
                settings::Outcome::Open => {}
                settings::Outcome::Cancelled => self.settings = None,
                settings::Outcome::Saved(cfg) => {
                    self.settings = None;
                    match &mut self.session {
                        Some(_) => {
                            // Network settings take effect on a fresh start.
                            self.shutdown();
                            if let Ok(exe) = std::env::current_exe() {
                                let _ = std::process::Command::new(exe).env(RESTART_ENV, "1").spawn();
                            }
                            self.closing = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            return;
                        }
                        None => self.start(cfg),
                    }
                }
            }
        }

        self.top_bar(ctx);
        self.status_bar(ctx);

        if self.session.is_none() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    if let Some(e) = &self.start_error {
                        ui.label(theme::head("EchoLink couldn't start", 24.0).color(theme::accent()));
                        ui.add_space(8.0);
                        ui.colored_label(ui.visuals().error_fg_color, e);
                        ui.add_space(12.0);
                        if ui.button("Open settings").clicked() {
                            self.settings = Some(settings::Form::new(config::load().ok().as_ref()));
                        }
                    } else {
                        ui.label(theme::silk("No station configured"));
                        ui.label(theme::head("Set up your station to begin.", 24.0).color(theme::accent()));
                    }
                });
            });
            return;
        }

        // Leave the right-hand panel room for its controls, even in a narrow tiled window.
        let width = ctx.screen_rect().width();
        let list_max = (width - 400.0).max(360.0);
        egui::SidePanel::left("stations")
            .resizable(true)
            .default_width((width * 0.55).min(600.0))
            .min_width(360.0_f32.min(list_max))
            .max_width(list_max)
            .show(ctx, |ui| {
                ui.set_opacity(theme::reveal(ctx, "list", 0.05));
                self.station_list(ui);
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.set_opacity(theme::reveal(ctx, "qso", 0.30));
            self.qso_panel(ui)
        });
        self.handle_ptt(ctx);
    }
}

impl Gui {
    fn top_bar(&mut self, ctx: &egui::Context) {
        let frame = egui::Frame::new()
            .fill(theme::chassis())
            .stroke(egui::Stroke::new(1.0, theme::edge()))
            .inner_margin(egui::Margin::symmetric(14, 8));
        egui::TopBottomPanel::top("top").frame(frame).show(ctx, |ui| {
            ui.set_opacity(theme::reveal(ctx, "top", 0.0));
            ui.horizontal(|ui| {
                ui.label(theme::head("ECHO", 24.0).color(theme::accent()));
                ui.add_space(-8.0);
                ui.label(theme::head("LINK", 24.0).color(theme::text()));
                if let Some(s) = &self.session {
                    ui.add_space(10.0);
                    // Callsign plate.
                    egui::Frame::new()
                        .fill(theme::screen())
                        .stroke(egui::Stroke::new(1.0, theme::accent_dim()))
                        .corner_radius(3)
                        .inner_margin(egui::Margin::symmetric(10, 3))
                        .show(ui, |ui| ui.label(theme::data(&s.cfg.callsign, 16.0).color(theme::accent())));
                    ui.add_space(6.0);
                    // A lamp, painted (no glyph needed) with a soft halo.
                    let color = if s.dir_ok { theme::rx() } else { theme::accent() };
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 8.0, color.gamma_multiply(0.18));
                    ui.painter().circle_filled(rect.center(), 4.5, color);
                    ui.label(RichText::new(&s.dir_status).color(theme::muted()).small());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("SETTINGS").clicked() {
                        let current = config::load().ok();
                        self.settings = Some(settings::Form::new(current.as_ref()));
                    }
                    if let Some(s) = &mut self.session
                        && ui.button("REFRESH LIST").clicked()
                    {
                        s.refresh();
                    }
                    ui.add_space(6.0);
                    if let Some(i) = theme::picker(ui) {
                        theme::set(ctx, i);
                    }
                });
            });
        });
    }

    fn status_bar(&self, ctx: &egui::Context) {
        let frame = egui::Frame::new()
            .fill(theme::chassis())
            .stroke(egui::Stroke::new(1.0, theme::edge()))
            .inner_margin(egui::Margin::symmetric(14, 5));
        egui::TopBottomPanel::bottom("status").frame(frame).show(ctx, |ui| {
            let line = self
                .session
                .as_ref()
                .and_then(|s| s.log.iter().rev().find(|(k, _)| *k == LogKind::System).map(|(_, t)| t.as_str()))
                .unwrap_or("");
            ui.label(RichText::new(format!("> {line}")).monospace().size(12.0).color(theme::accent_dim()));
        });
    }

    fn station_list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.search)
                        .font(egui::FontId::new(14.0, theme::mono_medium()))
                        .hint_text("search call / place / node")
                        .desired_width(250.0)
                        .margin(egui::Margin::symmetric(8, 5)),
                )
                .changed();
            if !self.search.is_empty() && ui.small_button("CLR").clicked() {
                self.search.clear();
                changed = true;
            }
            changed |= ui.toggle_value(&mut self.favorites_only, "★ FAVORITES").changed();
        });
        ui.horizontal(|ui| {
            for f in Filter::ALL {
                let on = self.filter == f;
                let label = if on {
                    theme::head(f.label().to_uppercase(), 13.0).color(theme::on_accent())
                } else {
                    theme::head(f.label().to_uppercase(), 13.0).color(theme::muted())
                };
                let b = egui::Button::new(label)
                    .fill(if on { theme::accent() } else { Color32::TRANSPARENT })
                    .stroke(egui::Stroke::NONE);
                if ui.add(b).clicked() {
                    self.filter = f;
                    changed = true;
                }
            }
        });
        if changed {
            self.refilter();
        }

        let Some(s) = &self.session else { return };
        if s.stations.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
                if s.dir_ok {
                    ui.label("Loading the station list…");
                } else if s.dir_status == "Error" {
                    ui.label("Couldn't reach the EchoLink directory. Trying again…");
                    if let Some((_, why)) = s.log.iter().rev().find(|(_, t)| t.starts_with("Directory: ")) {
                        ui.label(RichText::new(why.trim_start_matches("Directory: ")).small().weak());
                    }
                } else {
                    ui.label("Connecting to the EchoLink directory…");
                }
            });
            return;
        }
        ui.label(theme::silk(&format!("{} of {} stations on air", self.visible.len(), s.stations.len())));

        let favs: Vec<String> = self.favorites().to_vec();
        let mut clicked: Option<String> = None;
        let mut connect: Option<String> = None;
        let mut star: Option<String> = None;
        TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(26.0))
            .column(Column::initial(118.0))
            .column(Column::initial(76.0))
            .column(Column::remainder().clip(true))
            .column(Column::exact(48.0))
            .header(24.0, |mut h| {
                for t in ["", "Call", "Node", "Location", ""] {
                    h.col(|ui| {
                        ui.label(theme::silk(t));
                    });
                }
            })
            .body(|body| {
                body.rows(27.0, self.visible.len(), |mut row| {
                    let st = &s.stations[self.visible[row.index()]];
                    let is_fav = favs.contains(&st.callsign);
                    row.set_selected(self.selected.as_deref() == Some(st.callsign.as_str()));
                    row.col(|ui| {
                        let icon = if is_fav { RichText::new("★").color(theme::accent()) } else { RichText::new("☆").color(theme::edge()) };
                        if ui.add(egui::Label::new(icon).sense(egui::Sense::click())).on_hover_text("Favorite").clicked() {
                            star = Some(st.callsign.clone());
                        }
                    });
                    row.col(|ui| {
                        ui.label(theme::data(&st.callsign, 14.0).color(if is_fav { theme::accent() } else { theme::text() }));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(st.id.to_string()).monospace().size(12.5).color(theme::muted()));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&st.description).color(theme::text().gamma_multiply(0.85)));
                    });
                    row.col(|ui| {
                        if st.busy {
                            ui.label(theme::silk("busy").color(theme::tx()));
                        }
                    });
                    let r = row.response();
                    if r.double_clicked() {
                        connect = Some(st.callsign.clone());
                    } else if r.clicked() {
                        clicked = Some(st.callsign.clone());
                    }
                });
            });
        if let Some(c) = star {
            self.toggle_favorite(&c);
        }
        if let Some(c) = clicked {
            self.selected = Some(c);
        }
        if let Some(c) = connect {
            self.selected = Some(c.clone());
            if let Some(s) = &mut self.session
                && let Some(st) = s.find(&c).cloned()
            {
                s.connect(&st);
            }
        }
    }

    fn qso_panel(&mut self, ui: &mut egui::Ui) {
        let selected = self.selected_station().cloned();
        let Some(s) = &mut self.session else { return };
        ui.add_space(6.0);

        // The readout: who we're talking to, or the station picked in the list.
        let (rx_active, marker_id) = match &s.state {
            State::Connected { .. } => (s.receiving, selected.as_ref().map(|st| st.id)),
            _ => (false, selected.as_ref().map(|st| st.id)),
        };
        let _ = rx_active;
        let screen = egui::Frame::new()
            .fill(theme::screen())
            .stroke(egui::Stroke::new(1.0, theme::edge()))
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(16, 12));
        let resp = screen.show(ui, |ui| {
            ui.set_width(ui.available_width());
            let on_air = matches!(s.state, State::Connected { .. });
            match s.state.clone() {
                State::Connected { callsign, name, .. } => {
                    ui.label(theme::silk("Linked"));
                    ui.label(theme::head(&callsign, 44.0).color(theme::accent()));
                    let info = if s.remote_info.is_empty() { name } else { s.remote_info.clone() };
                    ui.label(RichText::new(info.trim()).color(theme::text().gamma_multiply(0.8)));
                    ui.add_space(4.0);
                    if ui.button("DISCONNECT").clicked() {
                        s.disconnect();
                    }
                }
                State::Connecting { label, .. } => {
                    ui.label(theme::silk("Calling"));
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(theme::head(&label, 30.0).color(theme::accent()));
                    });
                    ui.add_space(4.0);
                    if ui.button("CANCEL").clicked() {
                        s.disconnect();
                    }
                }
                State::Idle => match &selected {
                    Some(st) => {
                        ui.label(theme::silk(if st.busy { "Selected / busy" } else { "Selected" }));
                        ui.label(theme::head(&st.callsign, 44.0).color(theme::accent()));
                        ui.label(
                            RichText::new(format!("NODE {}   {}", st.id, st.description))
                                .monospace()
                                .size(12.5)
                                .color(theme::muted()),
                        );
                        ui.add_space(4.0);
                        if ui.button("CONNECT").clicked() {
                            s.connect(st);
                        }
                    }
                    None => {
                        ui.label(theme::silk("Standby"));
                        ui.label(theme::head("NOT CONNECTED", 34.0).color(theme::accent_dim()));
                        ui.label(RichText::new("Pick a station and double-click it, or dial a node number.").color(theme::muted()));
                    }
                },
            }
            ui.add_space(8.0);
            // The tuning scale parks its marker by node number: a stable "frequency" per station.
            theme::dial(ui, marker_id.map(|id| (id % 1000) as f32 / 1000.0), on_air);
        });
        theme::scanlines(&ui.painter_at(resp.response.rect.shrink(1.0)), resp.response.rect.shrink(1.0));

        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.node_input)
                    .font(egui::FontId::new(14.0, theme::mono_medium()))
                    .hint_text("node #")
                    .char_limit(7)
                    .margin(egui::Margin::symmetric(8, 5))
                    .desired_width(100.0),
            );
            self.node_input.retain(|c| c.is_ascii_digit());
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("CONNECT TO NODE").clicked() || enter)
                && let Ok(id) = self.node_input.parse()
            {
                s.connect_node(id);
            }
            if ui.button("ECHOTEST").on_hover_text("Records what you say and plays it back").clicked() {
                s.connect_node(ECHOTEST_NODE);
            }
        });

        ui.add_space(10.0);
        self.ptt_button(ui);

        // Audio meters and mic controls.
        ui.add_space(8.0);
        self.audio_panel(ui);
        let Some(s) = &self.session else { return };

        // Messages and chat.
        ui.add_space(10.0);
        ui.label(theme::silk("Station log"));
        let log_height = (ui.available_height() - 40.0).max(60.0);
        egui::Frame::new().fill(theme::screen()).stroke(egui::Stroke::new(1.0, theme::edge())).corner_radius(3).inner_margin(8).show(ui, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).max_height(log_height).show(ui, |ui| {
                for (kind, line) in &s.log {
                    match kind {
                        LogKind::Chat => ui.label(RichText::new(line).monospace().size(13.0).color(theme::accent())),
                        LogKind::System => ui.label(RichText::new(line).monospace().size(12.5).color(theme::muted())),
                    };
                }
            });
        });
        let connected = s.connected();
        let Some(s) = &mut self.session else { return };
        ui.horizontal(|ui| {
            let r = ui.add_enabled(
                connected,
                egui::TextEdit::singleline(&mut self.chat_input).font(egui::FontId::new(14.0, theme::mono_medium())).hint_text("chat message").margin(egui::Margin::symmetric(8, 5)).desired_width(ui.available_width() - 80.0),
            );
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.add_enabled(connected, egui::Button::new("SEND")).clicked() || enter) && !self.chat_input.trim().is_empty() {
                s.chat(&self.chat_input);
                self.chat_input.clear();
                r.request_focus();
            }
        });
    }

    fn ptt_button(&mut self, ui: &mut egui::Ui) {
        let Some(s) = &self.session else { return };
        let connected = s.connected();
        let now = ui.input(|i| i.time);
        let (text, sub, accent, hot) = match s.tx_since {
            Some(t) => {
                let secs = t.elapsed().as_secs();
                let warn = t.elapsed() > TX_LIMIT - Duration::from_secs(30);
                (
                    "ON AIR".to_string(),
                    format!("{}:{:02}{}", secs / 60, secs % 60, if warn { "  ·  RELEASE SOON" } else { "" }),
                    if warn { theme::accent() } else { theme::tx() },
                    true,
                )
            }
            None if s.receiving => ("RECEIVING".to_string(), "stand by".to_string(), theme::rx(), false),
            None if connected => ("PUSH TO TALK".to_string(), "hold here  ·  or hold SPACE".to_string(), theme::accent(), false),
            None => ("NO LINK".to_string(), "connect to a station to talk".to_string(), theme::edge(), false),
        };
        let size = egui::vec2(ui.available_width(), 88.0);
        let sense = if connected { egui::Sense::click_and_drag() } else { egui::Sense::hover() };
        let (rect, r) = ui.allocate_exact_size(size, sense);
        let down = connected && r.is_pointer_button_down_on();
        let p = ui.painter();
        // Transmit glow breathes; everything else is still.
        let pulse = if hot { 0.55 + 0.45 * ((now * 5.0).sin() as f32 * 0.5 + 0.5) } else { 0.0 };
        if hot {
            for i in 1..=5 {
                p.rect_stroke(
                    rect.expand(i as f32 * 2.5),
                    4.0 + i as f32 * 2.0,
                    egui::Stroke::new(2.0, accent.gamma_multiply(pulse * 0.22 / i as f32)),
                    egui::StrokeKind::Outside,
                );
            }
        }
        let fill = if hot {
            accent.gamma_multiply(0.22 + 0.12 * pulse)
        } else if down {
            theme::accent().gamma_multiply(0.25)
        } else if r.hovered() && connected {
            theme::pal().hover
        } else {
            theme::screen()
        };
        p.rect_filled(rect, 4.0, fill);
        p.rect_stroke(rect, 4.0, egui::Stroke::new(if hot || connected { 2.0 } else { 1.0 }, accent), egui::StrokeKind::Inside);
        // Corner screws, because it's hardware.
        for (dx, dy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            let c = egui::pos2(
                if dx > 0.0 { rect.left() + 9.0 } else { rect.right() - 9.0 },
                if dy > 0.0 { rect.top() + 9.0 } else { rect.bottom() - 9.0 },
            );
            p.circle_filled(c, 2.5, accent.gamma_multiply(0.35));
        }
        // Lamp.
        let lamp = egui::pos2(rect.left() + 38.0, rect.center().y);
        p.circle_filled(lamp, 15.0, accent.gamma_multiply(if hot { 0.25 * pulse + 0.1 } else { 0.1 }));
        p.circle_filled(lamp, 8.0, if hot || s.receiving || connected { accent } else { accent.gamma_multiply(0.4) });
        let tx = rect.left() + 68.0;
        p.text(egui::pos2(tx, rect.center().y - 11.0), egui::Align2::LEFT_CENTER, text, egui::FontId::new(30.0, theme::display()), if connected { accent } else { theme::muted() });
        p.text(egui::pos2(tx, rect.center().y + 20.0), egui::Align2::LEFT_CENTER, sub, egui::FontId::new(12.5, FontFamily::Monospace), theme::muted());
        self.ptt_held_by_mouse = down;
    }

    fn handle_ptt(&mut self, ctx: &egui::Context) {
        let Some(s) = &mut self.session else { return };
        // Space works while no text box has the keyboard.
        let typing = ctx.memory(|m| m.focused().is_some()) && ctx.wants_keyboard_input();
        let space = !typing && self.settings.is_none() && ctx.input(|i| i.key_down(egui::Key::Space));
        let want = (self.ptt_held_by_mouse || space) && s.connected();
        if want != self.ptt_on {
            self.ptt_on = want;
            s.ptt(want);
        }
    }
}
