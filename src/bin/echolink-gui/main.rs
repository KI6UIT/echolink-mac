//! EchoLink desktop app: the same station core as echolink-tui, with a
//! window, a big push-to-talk button and a settings dialog.

mod nets_tab;
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

fn split_path() -> std::path::PathBuf {
    echolink::config::path().with_file_name("gui-layout")
}

/// Saved window layout: the two dividers, and the order of the gauges.
struct Layout {
    top: f32,
    audio: f32,
    gauges: Vec<usize>,
}

const GAUGES: usize = 6; // RX, MIC, LINK, COMP, NR, link details

fn default_gauge_order() -> Vec<usize> {
    (0..GAUGES).collect()
}

impl Layout {
    fn load() -> Layout {
        let text = std::fs::read_to_string(split_path()).unwrap_or_default();
        let mut lines = text.lines();
        let mut nums = lines.next().unwrap_or("").split_whitespace().filter_map(|t| t.parse::<f32>().ok()).map(|v| v.clamp(0.05, 0.9));
        let (top, audio) = (nums.next().unwrap_or(0.30), nums.next().unwrap_or(0.32));
        let mut gauges: Vec<usize> = lines.next().unwrap_or("").split_whitespace().filter_map(|t| t.parse().ok()).collect();
        let mut sorted = gauges.clone();
        sorted.sort();
        if sorted != default_gauge_order() {
            gauges = default_gauge_order(); // missing, damaged or from another version
        }
        Layout { top, audio, gauges }
    }

    fn save(&self) {
        let order: Vec<String> = self.gauges.iter().map(|g| g.to_string()).collect();
        let _ = std::fs::write(split_path(), format!("{:.3} {:.3}\n{}\n", self.top, self.audio, order.join(" ")));
    }
}

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

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Station,
    Logbook,
    Nets,
}

/// The open QRZ profile window.
struct PageWin {
    call: String,
    state: PageState,
    rx: Option<std::sync::mpsc::Receiver<Result<echolink::lookup::Page, String>>>,
    /// The photo, shared so it isn't copied every frame.
    photo: Option<Arc<[u8]>>,
}

/// What to do after drawing the QRZ page.
#[derive(Default)]
struct PageActions {
    connect: Option<String>,
    block: Option<String>,
    close: bool,
}

impl PageActions {
    fn merge(&mut self, o: PageActions) {
        self.connect = self.connect.take().or(o.connect);
        self.block = self.block.take().or(o.block);
        self.close |= o.close;
    }
}

/// The "Edit contact" dialog.
struct EditForm {
    key: String,
    /// The contact's id when it was made in this app; `None` for one that came from QRZ.
    local_id: Option<String>,
    on_qrz: bool,
    /// Adding a new contact by hand rather than editing one.
    adding: bool,
    echolink: bool,
    call: String,
    name: String,
    date: String,
    time: String,
    duration: String,
    band: String,
    mode: String,
    error: String,
}

/// The "Delete contact" confirmation.
struct DelForm {
    key: String,
    call: String,
    when: String,
    on_qrz: bool,
}

enum PageState {
    Loading,
    Ready(Box<echolink::lookup::Page>),
    Failed(String),
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
    keypad_open: bool,
    tab: Tab,
    page: Option<PageWin>,
    nets: nets_tab::NetsUi,
    /// The selected logbook entry (its key), and the edit / delete dialogs.
    book_sel: Option<String>,
    edit: Option<EditForm>,
    del: Option<DelForm>,
    /// Shares of the right-hand column given to the readout and to the audio section (0..1);
    /// the logbook gets the rest. Both are set by dragging the dividers.
    split: f32,
    split2: f32,
    /// Which gauge is shown where; changed by dragging them.
    gauge_order: Vec<usize>,
    /// When the search box last changed, and the last callsign it looked up.
    search_at: Option<Instant>,
    search_looked: String,
    /// QRZ's session key, kept between lookups as QRZ asks.
    qrz_session: Arc<std::sync::Mutex<Option<String>>>,
    log_note: String,
    book_search: String,
    book_filtered: Vec<usize>,
    /// What `book_filtered` was computed for: search text and logbook generation.
    book_key: (String, u64),
    dtmf_input: String,
    /// True while the system-wide talk key is held (macOS).
    hotkey_down: Arc<AtomicBool>,
    #[cfg(target_os = "macos")]
    _hotkeys: Option<global_hotkey::GlobalHotKeyManager>,
}

impl Gui {
    fn new(cc: &eframe::CreationContext) -> Gui {
        theme::install(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
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
            keypad_open: false,
            tab: Tab::Station,
            page: None,
            nets: Default::default(),
            book_sel: None,
            edit: None,
            del: None,
            split: Layout::load().top,
            split2: Layout::load().audio,
            gauge_order: Layout::load().gauges,
            search_at: None,
            search_looked: String::new(),
            qrz_session: Arc::default(),
            log_note: String::new(),
            book_search: String::new(),
            book_filtered: Vec::new(),
            book_key: (String::from("\u{0}"), 0),
            dtmf_input: String::new(),
            hotkey_down: Arc::new(AtomicBool::new(false)),
            #[cfg(target_os = "macos")]
            _hotkeys: None,
        };
        match config::load() {
            Ok(cfg) => {
                if std::env::var_os(RESTART_ENV).is_some() {
                    // Give the previous instance a moment to release its ports.
                    std::thread::sleep(Duration::from_millis(1500));
                }
                #[cfg(target_os = "macos")]
                gui.register_hotkey(&cc.egui_ctx, cfg.ptt_hotkey.as_deref());
                gui.start(cfg);
            }
            Err(_) => gui.settings = Some(settings::Form::new(None)),
        }
        gui
    }

    /// Hold-to-talk from any app. Uses Carbon hotkeys, which need no
    /// Accessibility permission.
    #[cfg(target_os = "macos")]
    fn register_hotkey(&mut self, ctx: &egui::Context, spec: Option<&str>) {
        use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
        let Some(spec) = spec else { return };
        let Ok(key) = spec.parse::<HotKey>() else {
            echolink::log!("talk key {spec:?} isn't a key combination I know");
            return;
        };
        let Ok(manager) = GlobalHotKeyManager::new() else { return };
        if let Err(e) = manager.register(key) {
            echolink::log!("couldn't register the talk key {spec}: {e}");
            return;
        }
        let (down, ctx, id) = (self.hotkey_down.clone(), ctx.clone(), key.id());
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.id == id {
                down.store(e.state == HotKeyState::Pressed, Ordering::Relaxed);
                ctx.request_repaint(); // wake the UI even if the window is hidden
            }
        }));
        self._hotkeys = Some(manager);
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

    /// The logbook tab: your QRZ logbook and the contacts made here, searchable,
    /// with sync, ADIF export and redial.
    /// The logbook with the QRZ page beside it, once a callsign has been looked up.
    fn logbook_tab(&mut self, ui: &mut egui::Ui) {
        // The room below the divider; drag the divider to give the logbook more or less.
        let height = ui.available_height().max(120.0);
        let width = ui.available_width();
        ui.allocate_ui(egui::vec2(width, height), |ui| self.logbook_split(ui));
    }

    fn logbook_split(&mut self, ui: &mut egui::Ui) {
        if self.page.is_some() {
            let mut actions = PageActions::default();
            egui::SidePanel::right("qrz-pane")
                .resizable(true)
                .default_width((ui.available_width() * 0.45).clamp(280.0, 420.0))
                .min_width(240.0)
                .max_width((ui.available_width() * 0.7).max(260.0))
                .frame(
                    egui::Frame::new()
                        .fill(theme::screen())
                        .stroke(egui::Stroke::new(1.0, theme::edge()))
                        .corner_radius(4)
                        .inner_margin(egui::Margin::symmetric(10, 8)),
                )
                .show_inside(ui, |ui| {
                    let call = self.page.as_ref().map(|p| p.call.clone()).unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.label(theme::silk("QRZ"));
                        ui.label(theme::data(&call, 14.0).color(theme::accent()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("CLOSE").clicked() {
                                actions.close = true;
                            }
                        });
                    });
                    ui.separator();
                    actions.merge(self.page_body(ui));
                });
            self.apply_page_actions(actions);
        }
        egui::CentralPanel::default().frame(egui::Frame::new()).show_inside(ui, |ui| self.logbook_main(ui));
    }

    /// STATION LOG | LOGBOOK.
    fn tab_row(tab: &mut Tab, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if theme::tab_button(ui, "STATION LOG", *tab == Tab::Station) {
                *tab = Tab::Station;
            }
            if theme::tab_button(ui, "LOGBOOK", *tab == Tab::Logbook) {
                *tab = Tab::Logbook;
            }
            if theme::tab_button(ui, "NETS", *tab == Tab::Nets) {
                *tab = Tab::Nets;
            }
        });
    }

    fn logbook_main(&mut self, ui: &mut egui::Ui) {
        use echolink::contacts::{Qrz, duration_label, utc_label};
        use echolink::logbook::EntryState;
        use echolink::session::SyncState;
        let Some(s) = &self.session else { return };
        let (mut sync, mut full, mut export, mut upload, mut add) = (false, false, false, false, false);
        let mut info: Option<String> = None;

        let has_key = s.qrz_key().is_some();
        let running = matches!(s.sync, SyncState::Running(_));
        let waiting = s.contacts.iter().filter(|c| c.is_qso() && matches!(c.qrz, Qrz::Pending | Qrz::Failed(_))).count();
        ui.horizontal_wrapped(|ui| {
            sync = ui
                .add_enabled(has_key && !running, egui::Button::new(if running { "SYNCING…" } else { "SYNC FROM QRZ" }))
                .on_hover_text("Copies your QRZ logbook here so you can see who you've worked before")
                .clicked();
            full = ui
                .add_enabled(has_key && !running, egui::Button::new("FULL RE-SYNC").small())
                .on_hover_text("Downloads everything again and removes contacts you deleted on QRZ")
                .clicked();
            add = ui.button("+ ADD CONTACT").on_hover_text("Log a contact by hand").clicked();
            export = ui.button("EXPORT ADIF").on_hover_text("Saves an .adi file in Documents that any logging program can import").clicked();
            upload = ui
                .add_enabled(has_key && waiting > 0, egui::Button::new(format!("SEND {waiting} TO QRZ")).small())
                .on_hover_text("Contacts made here that QRZ doesn't have yet")
                .clicked();
        });
        let status = match &s.sync {
            _ if !has_key => "Add your QRZ logbook key in Settings to sync.".to_string(),
            SyncState::Running(n) => format!("Syncing with QRZ… {n} new so far"),
            SyncState::Done { new, total } => format!("Synced: {total} contacts ({new} new)"),
            SyncState::Failed(e) => format!("Sync stopped: {e}"),
            SyncState::Idle if s.last_sync.is_none() => "Not synced yet. Press SYNC FROM QRZ to copy your logbook here.".to_string(),
            SyncState::Idle => format!("{} contacts", s.book.len()),
        };
        let when = s.last_sync.map(|t| format!("  ·  last sync {} UTC", utc_label(t))).unwrap_or_default();
        let color = if matches!(s.sync, SyncState::Failed(_)) { theme::tx() } else { theme::muted() };
        ui.label(RichText::new(format!("{status}{when}")).small().color(color));
        if !self.log_note.is_empty() {
            ui.label(RichText::new(&self.log_note).small().color(theme::accent()));
        }

        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.book_search)
                    .hint_text("search, or type a callsign to see its QRZ page")
                    .desired_width((ui.available_width() - 140.0).clamp(110.0, 290.0)),
            );
            if r.changed() {
                self.search_at = Some(Instant::now());
            }
            if ui.add_enabled(!self.book_search.is_empty(), egui::Button::new("CLR")).on_hover_text("Clear the search").clicked() {
                self.book_search.clear();
                self.search_at = None;
                self.search_looked.clear(); // so typing the same callsign again looks it up again
                r.request_focus();
            }
            // A callsign typed in gets looked up once you pause (or press Enter), not on every key.
            let typed = self.book_search.trim().to_uppercase();
            let callish = echolink::lookup::looks_like_call(&typed);
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let paused = self.search_at.is_some_and(|t| t.elapsed() >= Duration::from_millis(700));
            if callish && typed != self.search_looked && (enter || (paused && typed.len() >= 4)) {
                info = Some(typed.clone());
            }
            if paused {
                self.search_at = None;
            } else if self.search_at.is_some() {
                ui.ctx().request_repaint_after(Duration::from_millis(250));
            }
            ui.label(RichText::new(format!("{} of {}", self.book_filtered.len(), s.book.len())).small().color(theme::muted()));
        });
        let (mut edit_req, mut del_req, mut sel_req, mut restore) = (None::<String>, None::<String>, None::<String>, false);
        ui.horizontal(|ui| {
            match self.book_sel.as_ref().and_then(|k| s.book.iter().find(|e| &e.key == k)) {
                Some(e) => {
                    ui.label(RichText::new(format!("{}  {}", e.call, echolink::contacts::date_str(e.when))).monospace().size(12.5).color(theme::accent()));
                    if ui.small_button("EDIT").on_hover_text("Change this contact").clicked() {
                        edit_req = Some(e.key.clone());
                    }
                    if ui.small_button("DELETE").on_hover_text("Remove this contact").clicked() {
                        del_req = Some(e.key.clone());
                    }
                }
                None => {
                    ui.label(RichText::new("Click a contact to look it up, edit it or delete it.").small().color(theme::muted()));
                }
            }
            if !s.hidden.is_empty() && ui.small_button(format!("RESTORE {} HIDDEN", s.hidden.len())).on_hover_text("Contacts you removed here only. Restoring lets a full re-sync bring them back.").clicked() {
                restore = true;
            }
        });
        let key = (self.book_search.trim().to_uppercase(), s.book_gen);
        if key != self.book_key {
            self.book_filtered = s
                .book
                .iter()
                .enumerate()
                .filter(|(_, e)| key.0.is_empty() || e.call.to_uppercase().contains(&key.0) || e.name.to_uppercase().contains(&key.0))
                .map(|(i, _)| i)
                .collect();
            self.book_key = key;
        }

        ui.add_space(4.0);
        let table_height = ui.available_height().max(120.0);
        // Squeezed beside the QRZ page, keep what matters: when, who, name, and whether QRZ has it.
        let compact = ui.available_width() < 540.0;
        let selected = self.book_sel.clone();
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(if compact { 104.0 } else { 112.0 }))
            .column(Column::exact(96.0));
        table = if compact {
            table.column(Column::remainder().clip(true)).column(Column::exact(58.0))
        } else {
            table
                .column(Column::initial(120.0).clip(true))
                .column(Column::exact(70.0))
                .column(Column::exact(52.0))
                .column(Column::remainder().clip(true))
        };
        table
            .max_scroll_height(table_height)
            .header(22.0, |mut h| {
                let heads: &[&str] = if compact { &["UTC", "Station", "Name", "QRZ"] } else { &["UTC", "Station", "Name", "Band/Mode", "Time", "QRZ"] };
                for t in heads {
                    h.col(|ui| {
                        ui.label(theme::silk(t));
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, self.book_filtered.len(), |mut row| {
                    let e = &s.book[self.book_filtered[row.index()]];
                    row.set_selected(selected.as_deref() == Some(e.key.as_str()));
                    row.col(|ui| {
                        ui.label(RichText::new(utc_label(e.when)).monospace().size(12.5));
                    });
                    row.col(|ui| {
                        let who = if e.incoming { format!("{} (in)", e.call) } else { e.call.clone() };
                        ui.label(RichText::new(who).monospace().color(theme::accent()));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&e.name).size(12.5));
                    });
                    if !compact {
                        row.col(|ui| {
                            ui.label(RichText::new(format!("{} {}", e.band, e.mode).trim()).small().color(theme::muted()));
                        });
                        row.col(|ui| {
                            if let Some(secs) = e.secs {
                                ui.label(RichText::new(duration_label(secs)).monospace().size(12.5));
                            }
                        });
                    }
                    row.col(|ui| {
                        let (text, short, color) = match &e.state {
                            EntryState::OnQrz => ("on QRZ".to_string(), "on QRZ", theme::rx()),
                            EntryState::Local(Qrz::Sent(_)) => ("sent".to_string(), "sent", theme::rx()),
                            EntryState::Local(Qrz::Failed(why)) => (format!("failed: {why}"), "failed", theme::tx()),
                            EntryState::Local(_) => ("not sent".to_string(), "not sent", theme::muted()),
                        };
                        ui.label(RichText::new(if compact { short.to_string() } else { text.clone() }).small().color(color)).on_hover_text(text);
                    });
                    let resp = row.response();
                    if resp.clicked() {
                        sel_req = Some(e.key.clone());
                        info = Some(e.call.clone());
                    }
                    resp.context_menu(|ui| {
                        sel_req = Some(e.key.clone());
                        if ui.button("Look up on QRZ").clicked() {
                            info = Some(e.call.clone());
                            ui.close_menu();
                        }
                        if ui.button("Edit…").clicked() {
                            edit_req = Some(e.key.clone());
                            ui.close_menu();
                        }
                        if ui.button("Delete…").clicked() {
                            del_req = Some(e.key.clone());
                            ui.close_menu();
                        }
                    });
                });
            });

        if let Some(k) = sel_req {
            self.book_sel = Some(k);
        }
        if add {
            self.open_add();
        }
        if let Some(k) = edit_req {
            self.open_edit(&k);
        }
        if let Some(k) = del_req {
            self.open_delete(&k);
        }
        let Some(s) = &mut self.session else { return };
        if restore {
            s.restore_hidden();
        }
        if sync || full {
            s.sync_qrz(full);
            self.log_note.clear();
        }
        if upload {
            let n = s.upload_pending();
            self.log_note = format!("Sending {n} contact(s) to QRZ…");
        }
        if export {
            self.log_note = match s.export_adif() {
                Ok(path) => {
                    let _ = std::process::Command::new("open").arg("-R").arg(&path).spawn(); // show it in Finder
                    format!("Saved {}", path.display())
                }
                Err(e) => format!("Couldn't save the file: {e}"),
            };
        }
        if let Some(call) = info {
            let ctx = ui.ctx().clone();
            self.start_lookup(&ctx, &call);
        }
    }

    fn open_edit(&mut self, key: &str) {
        let Some(s) = &self.session else { return };
        let Some(e) = s.book.iter().find(|e| e.key == key) else { return };
        let local = key.strip_prefix("L:").and_then(|id| s.contacts.iter().find(|c| c.id() == id));
        self.edit = Some(match local {
            Some(c) => EditForm {
                key: key.to_string(),
                local_id: Some(c.id()),
                on_qrz: e.logid.is_some(),
                adding: false,
                echolink: c.echolink,
                call: c.call.clone(),
                name: c.name.clone(),
                date: echolink::contacts::date_str(c.start),
                time: echolink::contacts::time_str(c.start),
                duration: echolink::contacts::duration_label(c.secs),
                band: c.band.clone().unwrap_or_else(|| s.cfg.log_band.clone()),
                mode: c.mode.clone().unwrap_or_else(|| s.cfg.log_mode.clone()),
                error: String::new(),
            },
            None => EditForm {
                key: key.to_string(),
                local_id: None,
                on_qrz: true,
                adding: false,
                echolink: true,
                call: e.call.clone(),
                name: e.name.clone(),
                date: echolink::contacts::date_str(e.when),
                time: echolink::contacts::time_str(e.when),
                duration: String::new(),
                band: e.band.clone(),
                mode: e.mode.clone(),
                error: String::new(),
            },
        });
    }

    /// A blank "Add contact" form, set to now.
    fn open_add(&mut self) {
        let Some(s) = &self.session else { return };
        let now = echolink::contacts::now();
        self.edit = Some(EditForm {
            key: String::new(),
            local_id: None,
            on_qrz: false,
            adding: true,
            echolink: true,
            call: String::new(),
            name: String::new(),
            date: echolink::contacts::date_str(now),
            time: echolink::contacts::time_str(now)[..5].to_string(),
            duration: "5:00".into(),
            band: s.cfg.log_band.clone(),
            mode: s.cfg.log_mode.clone(),
            error: String::new(),
        });
    }

    fn open_delete(&mut self, key: &str) {
        let Some(s) = &self.session else { return };
        let Some(e) = s.book.iter().find(|e| e.key == key) else { return };
        self.del = Some(DelForm {
            key: key.to_string(),
            call: e.call.clone(),
            when: echolink::contacts::utc_label(e.when),
            on_qrz: e.logid.is_some(),
        });
    }

    /// Edit a contact made in this app. Contacts that came from QRZ are edited on QRZ.
    fn edit_dialog(&mut self, ctx: &egui::Context) {
        let Some(f) = &mut self.edit else { return };
        let (mut close, mut save) = (false, false);
        let title = if f.adding { "Add contact" } else { "Edit contact" };
        egui::Window::new(title).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.set_width(360.0);
            if f.local_id.is_none() && !f.adding {
                ui.label(format!("{} is in your QRZ logbook.", f.call));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "QRZ keeps more about a contact than shows here (signal reports, notes, confirmations). Saving a copy from this app would wipe those, so contacts that came from QRZ are edited on QRZ. Then press SYNC FROM QRZ to bring the changes here.",
                    )
                    .small()
                    .color(theme::muted()),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("OPEN QRZ LOGBOOK").clicked() {
                        let _ = std::process::Command::new("open").arg(echolink::qrz::LOGBOOK_URL).spawn();
                    }
                    if ui.button("CLOSE").clicked() {
                        close = true;
                    }
                });
                return;
            }
            egui::Grid::new("edit-contact").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                let mut field = |ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str, width: f32| {
                    ui.label(label);
                    ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(width));
                    ui.end_row();
                };
                field(ui, "Station", &mut f.call, "K6ABC", 140.0);
                field(ui, "Name", &mut f.name, "", 220.0);
                field(ui, "Date (UTC)", &mut f.date, "2026-11-09", 110.0);
                field(ui, "Time (UTC)", &mut f.time, "18:04 or 18:04:09", 130.0);
                field(ui, "Length", &mut f.duration, "12:30", 90.0);
                field(ui, "Band", &mut f.band, "2m", 70.0);
                field(ui, "Mode", &mut f.mode, "FM", 70.0);
                ui.label("");
                ui.checkbox(&mut f.echolink, "EchoLink contact");
                ui.end_row();
            });
            if !f.echolink {
                ui.label(RichText::new("Not tagged as EchoLink in the ADIF, for a contact made another way.").small().color(theme::muted()));
            }
            if f.on_qrz {
                ui.add_space(4.0);
                ui.label(RichText::new("This contact is on QRZ. Saving uploads the edited version and then removes the old copy from QRZ.").small().color(theme::muted()));
            }
            if !f.error.is_empty() {
                ui.colored_label(ui.visuals().error_fg_color, &f.error);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                save = ui.button(if f.adding { "ADD" } else { "SAVE" }).clicked();
                if ui.button("CANCEL").clicked() {
                    close = true;
                }
            });
        });
        if save {
            let check = (|| {
                let call = f.call.trim().to_uppercase();
                if call.is_empty() || call.contains(char::is_whitespace) || call.len() > 24 {
                    return Err("Enter the station's callsign.".to_string());
                }
                let start = echolink::contacts::parse_datetime(&f.date, &f.time).ok_or("Date must look like 2026-11-09 and time like 18:04 (UTC).")?;
                let secs = echolink::contacts::parse_duration(&f.duration).ok_or("Length must look like 12:30 (minutes:seconds).")?;
                if secs < echolink::contacts::MIN_QSO_SECS || call.starts_with('*') {
                    return Err("Contacts shorter than 10 seconds, and conferences, aren't logged. Delete it instead.".to_string());
                }
                if f.band.trim().is_empty() || f.mode.trim().is_empty() {
                    return Err("QRZ needs a band and a mode.".to_string());
                }
                Ok(echolink::session::ContactEdit { call, name: f.name.clone(), start, secs, band: f.band.clone(), mode: f.mode.clone(), echolink: f.echolink })
            })();
            match (check, &mut self.session) {
                (Err(why), _) => f.error = why,
                (Ok(e), Some(s)) => {
                    let result = if f.adding {
                        s.add_contact(e)
                    } else if let Some(id) = &f.local_id {
                        s.edit_contact(id, e)
                    } else {
                        Ok(())
                    };
                    match result {
                        Ok(()) => close = true,
                        Err(why) => f.error = why,
                    }
                }
                _ => {}
            }
        }
        if close {
            self.edit = None;
            self.book_sel = None; // the edited entry may have a new id
        }
    }

    /// Confirm before anything is deleted, and say plainly what happens on QRZ.
    fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(f) = &self.del else { return };
        let has_key = self.session.as_ref().is_some_and(|s| s.qrz_key().is_some());
        let (mut here, mut everywhere, mut cancel) = (false, false, false);
        egui::Window::new("Delete contact").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new(format!("{}   {} UTC", f.call, f.when)).monospace().color(theme::accent()));
            ui.add_space(6.0);
            if f.on_qrz {
                ui.label("This contact is in your QRZ logbook. What do you want to do?");
                ui.add_space(4.0);
                ui.label(RichText::new("REMOVE FROM THIS APP ONLY hides it here. It stays on QRZ, and re-syncing won't bring it back.").small().color(theme::muted()));
                ui.label(
                    RichText::new("DELETE FROM QRZ TOO deletes it from your QRZ logbook for good. QRZ has no undo.").small().color(theme::tx()),
                );
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    here = ui.button("REMOVE FROM THIS APP ONLY").clicked();
                    everywhere = ui
                        .add_enabled(has_key, egui::Button::new(RichText::new("DELETE FROM QRZ TOO").color(theme::tx())))
                        .on_disabled_hover_text("Add your QRZ logbook key in Settings first")
                        .clicked();
                    cancel = ui.button("CANCEL").clicked();
                });
            } else {
                ui.label("Delete this contact from your logbook? It also leaves the ADIF export. This can't be undone.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    here = ui.button(RichText::new("DELETE").color(theme::tx())).clicked();
                    cancel = ui.button("CANCEL").clicked();
                });
            }
        });
        let key = f.key.clone();
        if here || everywhere {
            if let Some(s) = &mut self.session {
                s.delete_entry(&key, everywhere);
            }
        }
        if here || everywhere || cancel {
            self.del = None;
            self.book_sel = None;
        }
    }

    /// Fetch a station's QRZ page in the background and open its window.
    fn start_lookup(&mut self, ctx: &egui::Context, call: &str) {
        let Some(s) = &self.session else { return };
        let call = echolink::lookup::lookup_call(call);
        self.search_looked = call.clone();
        let (user, pass) = (s.cfg.qrz_user.clone().unwrap_or_default(), s.cfg.qrz_pass.clone().unwrap_or_default());
        if user.trim().is_empty() || pass.is_empty() {
            let why = "To look up a callsign, add your QRZ username and password in Settings (under QRZ logbook, Lookups).";
            self.page = Some(PageWin { call, state: PageState::Failed(why.into()), rx: None, photo: None });
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (session, repaint, who) = (self.qrz_session.clone(), ctx.clone(), call.clone());
        std::thread::spawn(move || {
            let _ = tx.send(echolink::lookup::Client::new(&user, &pass, session).lookup(&who));
            repaint.request_repaint();
        });
        self.page = Some(PageWin { call, state: PageState::Loading, rx: Some(rx), photo: None });
    }

    /// Pick up the QRZ answer when it arrives.
    fn poll_page(&mut self) {
        let Some(win) = &mut self.page else { return };
        if let Some(rx) = &win.rx
            && let Ok(answer) = rx.try_recv()
        {
            win.rx = None;
            win.state = match answer {
                Ok(page) => {
                    win.photo = page.image.as_ref().map(|b| Arc::from(b.as_slice()));
                    PageState::Ready(Box::new(page))
                }
                Err(e) => PageState::Failed(e),
            };
        }
    }

    fn apply_page_actions(&mut self, a: PageActions) {
        if a.close {
            self.page = None;
        }
        if let Some(s) = &mut self.session {
            if let Some(c) = a.connect {
                s.connect_call(&c);
            }
            if let Some(c) = a.block {
                s.block(&c);
            }
        }
    }

    /// What's on someone's QRZ page, plus your history with them.
    fn page_body(&mut self, ui: &mut egui::Ui) -> PageActions {
        let mut actions = PageActions::default();
        let (Some(win), Some(s)) = (&self.page, &self.session) else { return actions };
        let call = win.call.clone();
        let web = format!("https://www.qrz.com/db/{call}");
        match &win.state {
            PageState::Loading => {
                ui.add_space(20.0);
                ui.label(RichText::new("Asking QRZ…").color(theme::accent()));
            }
            PageState::Failed(why) => {
                ui.add_space(8.0);
                ui.colored_label(ui.visuals().error_fg_color, why);
            }
            PageState::Ready(p) => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).max_height((ui.available_height() - 44.0).max(120.0)).show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        if let Some(photo) = &win.photo {
                            ui.add(egui::Image::from_bytes(format!("bytes://qrz-{call}"), photo.clone()).max_width(150.0).max_height(200.0).corner_radius(4));
                        }
                        ui.vertical(|ui| {
                            ui.label(theme::head(p.get("call"), 24.0).color(theme::accent()));
                            let name = p.full_name();
                            if !name.is_empty() {
                                ui.label(RichText::new(name).size(15.0));
                            }
                            for line in p.address() {
                                ui.label(RichText::new(line).color(theme::text().gamma_multiply(0.9)));
                            }
                        });
                    });
                    if let Some(n) = &p.notice {
                        ui.add_space(4.0);
                        ui.label(RichText::new(n).small().color(theme::tx()));
                    }
                    ui.add_space(6.0);
                    egui::Grid::new("qrz-fields").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                        let mut row = |label: &str, value: String| {
                            if !value.trim().is_empty() {
                                ui.label(theme::silk(label));
                                ui.label(RichText::new(value).monospace().size(12.5));
                                ui.end_row();
                            }
                        };
                        row("Class", p.get("class").to_string());
                        let lic = [p.get("efdate"), p.get("expdate")];
                        row("Licensed", if lic[0].is_empty() && lic[1].is_empty() { String::new() } else { format!("{} to {}", lic[0], lic[1]) });
                        row("Also known as", p.get("aliases").to_string());
                        row("Previous call", p.get("p_call").to_string());
                        row("Grid", p.get("grid").to_string());
                        row("County", p.get("county").to_string());
                        row("Country", p.get("land").to_string());
                        row("Lat / Lon", if p.get("lat").is_empty() { String::new() } else { format!("{}, {}", p.get("lat"), p.get("lon")) });
                        row("CQ / ITU zone", if p.get("cqzone").is_empty() { String::new() } else { format!("{} / {}", p.get("cqzone"), p.get("ituzone")) });
                        row("QSL manager", p.get("qslmgr").to_string());
                        row("eQSL / LoTW / paper", [p.get("eqsl"), p.get("lotw"), p.get("mqsl")].iter().map(|v| if v.is_empty() { "?" } else { v }).collect::<Vec<_>>().join(" / "));
                        row("Email", p.get("email").to_string());
                        row("QRZ page views", p.get("u_views").to_string());
                        row("Born", p.get("born").to_string());
                        row("Updated", p.get("moddate").to_string());
                    });
                    let site = p.get("url");
                    if site.starts_with("https://") || site.starts_with("http://") {
                        ui.add_space(4.0);
                        if ui.link(site).on_hover_text("Open in your browser").clicked() {
                            let _ = std::process::Command::new("open").arg(site).spawn();
                        }
                    }

                    // What you've done with them.
                    ui.add_space(8.0);
                    ui.separator();
                    ui.label(theme::silk("YOUR HISTORY WITH THEM"));
                    let who = echolink::logbook::norm(&call);
                    match s.worked.get(&who) {
                        Some(w) => {
                            ui.label(format!("Worked {} time{}; last {} UTC", w.count, if w.count == 1 { "" } else { "s" }, echolink::contacts::utc_label(w.last)));
                            for e in s.book.iter().filter(|e| echolink::logbook::norm(&e.call) == who).take(8) {
                                ui.label(RichText::new(format!("{}  {} {}", echolink::contacts::utc_label(e.when), e.band, e.mode)).monospace().size(12.0).color(theme::muted()));
                            }
                        }
                        None => {
                            ui.label(RichText::new("Not in your logbook yet.").color(theme::muted()));
                        }
                    }

                    if let Some(bio) = &p.bio {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.label(theme::silk("BIO"));
                        ui.label(RichText::new(bio).size(13.0));
                    }
                });
            }
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button("OPEN ON QRZ.COM").clicked() && echolink::lookup::looks_like_call(&call) {
                let _ = std::process::Command::new("open").arg(&web).spawn();
            }
            let online = [call.clone(), format!("{call}-R"), format!("{call}-L")].into_iter().find(|c| s.find(c).is_some());
            if ui.add_enabled(online.is_some(), egui::Button::new("CONNECT")).on_hover_text("Call them on EchoLink if they're online").clicked() {
                actions.connect = online;
            }
            if ui.button("BLOCK").on_hover_text("Never accept calls from them").clicked() {
                actions.block = Some(call.clone());
            }
        });
        actions
    }

    /// Save the mic settings onto whatever is on disk.
    fn save_mic_settings(&mut self) {
        let Some(s) = &mut self.session else { return };
        s.cfg.mic_gain_db = s.audio.shared.gain_db();
        s.cfg.auto_gain = s.audio.shared.auto_gain.load(Ordering::Relaxed);
        s.cfg.noise_reduction = s.audio.shared.denoise.load(Ordering::Relaxed);
        s.cfg.speaker_volume = s.audio.shared.volume();
        if let Ok(mut on_disk) = config::load() {
            on_disk.mic_gain_db = s.cfg.mic_gain_db;
            on_disk.auto_gain = s.cfg.auto_gain;
            on_disk.noise_reduction = s.cfg.noise_reduction;
            on_disk.speaker_volume = s.cfg.speaker_volume;
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
        let nr = shared.nr_db_x10.load(Ordering::Relaxed) as f32 / 10.0 / 30.0;
        let comp = shared.comp_db_x10.load(Ordering::Relaxed) as f32 / 10.0 / 12.0;
        // Connection health, from the other station's packets.
        let linked = s.connected();
        let age = shared.link_age_ms.load(Ordering::Relaxed) as f32 / 1000.0;
        let voice_age = shared.link_voice_age_ms.load(Ordering::Relaxed);
        let recent = voice_age != u32::MAX && voice_age < 20_000;
        let loss = shared.link_loss_permille.load(Ordering::Relaxed) as f32 / 10.0;
        let jitter = shared.link_jitter_x10.load(Ordering::Relaxed) as f32 / 10.0;
        let quality = echolink::link::quality(age, loss, jitter, recent);
        let grade_color = |q: f32| if q >= 0.8 { theme::rx() } else if q >= 0.5 { theme::warn() } else { theme::tx() };
        let route = if s.cfg.proxy.is_some() { "via proxy" } else { "direct" };
        let card = if linked {
            let heard = if age < 1.5 { "just now".to_string() } else { format!("{age:.0} s ago") };
            theme::Card {
                title: format!("LINK {}", echolink::link::grade(quality)),
                title_color: grade_color(quality),
                lines: vec![
                    (if shared.link_voice_age_ms.load(Ordering::Relaxed) == u32::MAX { "Loss -   Jitter -".to_string() } else { format!("Loss {loss:.1}%  Jitter {jitter:.0} ms") }, theme::text()),
                    (format!("Heard {heard}"), theme::muted()),
                    (route.to_string(), theme::muted()),
                ],
            }
        } else {
            theme::Card {
                title: "LINK".into(),
                title_color: theme::muted(),
                lines: vec![("Not connected".to_string(), theme::muted()), (format!("Directory {}", s.dir_status.split(" · ").next().unwrap_or("")), theme::muted()), (route.to_string(), theme::muted())],
            }
        };
        let mut by_id: Vec<Option<theme::Cell>> = vec![
            Some(theme::Cell::Gauge(theme::Gauge { label: "RX", value: rx, scale: theme::Scale::Level, active: true })),
            Some(theme::Cell::Gauge(theme::Gauge { label: "MIC", value: mic, scale: theme::Scale::Level, active: true })),
            Some(theme::Cell::Gauge(theme::Gauge { label: "LINK", value: quality, scale: theme::Scale::Quality, active: linked })),
            Some(theme::Cell::Gauge(theme::Gauge { label: "COMP", value: comp, scale: theme::Scale::Reduction, active: shared.auto_gain.load(Ordering::Relaxed) })),
            Some(theme::Cell::Gauge(theme::Gauge { label: "NR", value: nr, scale: theme::Scale::Noise, active: shared.denoise.load(Ordering::Relaxed) })),
            Some(theme::Cell::Card(card)),
        ];
        let cells: Vec<theme::Cell> = self.gauge_order.iter().filter_map(|&id| by_id.get_mut(id).and_then(Option::take)).collect();
        let ev = theme::gauge_grid(ui, &cells);
        if let Some((from, to)) = ev.moved {
            let id = self.gauge_order.remove(from);
            self.gauge_order.insert(to, id);
            self.save_layout();
        }
        if ev.reset {
            self.gauge_order = default_gauge_order();
            self.save_layout();
        }

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
            ui.add_sized([26.0, 18.0], egui::Label::new(theme::silk("VOL")));
            let mut pct = shared.volume() * 100.0;
            let r = ui.add(egui::Slider::new(&mut pct, 0.0..=echolink::audio::MAX_VOLUME * 100.0).suffix("%").fixed_decimals(0));
            if r.changed() {
                shared.set_volume(pct / 100.0);
            }
            save |= r.drag_stopped() || (r.changed() && !r.dragged());
        });
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

    /// Touch-tone keypad for repeater and conference control.
    fn keypad(&mut self, ui: &mut egui::Ui) {
        let Some(s) = &mut self.session else { return };
        let connected = s.connected();
        let open = self.keypad_open;
        if theme::section_toggle(ui, "DTMF KEYPAD", open) {
            self.keypad_open = !open;
        }
        if !self.keypad_open {
            return;
        }
        ui.add_enabled_ui(connected, |ui| {
            egui::Grid::new("dtmf").spacing([6.0, 6.0]).show(ui, |ui| {
                for row in ["123A", "456B", "789C", "*0#D"] {
                    for key in row.chars() {
                        let b = ui.add_sized([44.0, 34.0], egui::Button::new(RichText::new(key.to_string()).monospace().size(16.0)));
                        if b.clicked() {
                            s.send_dtmf(&key.to_string());
                        }
                    }
                    ui.end_row();
                }
            });
            ui.horizontal(|ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.dtmf_input).hint_text("digits, e.g. *1234#").desired_width(150.0),
                );
                self.dtmf_input.retain(echolink::dtmf::valid);
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("SEND").clicked() || enter) && !self.dtmf_input.is_empty() {
                    s.send_dtmf(&self.dtmf_input);
                    self.dtmf_input.clear();
                }
            });
        });
        if !connected {
            ui.label(RichText::new("Connect to a station to send tones.").small().color(theme::muted()));
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
        let deadline = std::time::Instant::now() + Duration::from_secs(25); // log-off is quick; QRZ uploads can take a few seconds
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
        if let Some(s) = &mut self.session
            && s.incoming.take().is_some()
            && !self.focused.load(Ordering::Relaxed)
        {
            // Bounce the Dock icon until you look.
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Critical));
        }

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
        self.poll_page();
        self.edit_dialog(ctx);
        self.delete_dialog(ctx);

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
            self.qso_panel(ui);
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
                    if self.session.is_some() && ui.button("LOGBOOK").on_hover_text("Your contacts, QRZ sync and ADIF export").clicked() {
                        self.tab = Tab::Logbook;
                    }
                    if let Some(s) = &mut self.session {
                        let on = s.cfg.dnd;
                        let label = RichText::new("DND").color(if on { theme::tx() } else { theme::text() });
                        if ui
                            .button(label)
                            .on_hover_text("Do Not Disturb: refuse every incoming call")
                            .clicked()
                        {
                            s.set_dnd(!on);
                        }
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
            .column(Column::exact(40.0))
            .column(Column::initial(76.0))
            .column(Column::remainder().clip(true))
            .column(Column::exact(48.0))
            .header(24.0, |mut h| {
                for t in ["", "Call", "Wkd", "Node", "Location", ""] {
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
                        if let Some(w) = s.worked.get(&echolink::logbook::norm(&st.callsign)) {
                            ui.label(RichText::new(format!("{}\u{d7}", w.count)).small().color(theme::accent_dim())).on_hover_text(format!(
                                "You've worked {} {} time{}; last {} UTC",
                                echolink::logbook::norm(&st.callsign),
                                w.count,
                                if w.count == 1 { "" } else { "s" },
                                echolink::contacts::utc_label(w.last)
                            ));
                        }
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

    /// The right-hand column in three sections with draggable dividers between them:
    /// readout and talk key, audio, then the station log or logbook.
    fn qso_panel(&mut self, ui: &mut egui::Ui) {
        let total = ui.available_height();
        let width = ui.available_width();
        let grip = 10.0;
        let avail = (total - 2.0 * grip).max(1.0);
        let (min_top, min_audio, min_bottom) = (90.0, 90.0, 150.0);
        let top = (self.split * avail).clamp(min_top, (avail - min_audio - min_bottom).max(min_top));
        let audio = (self.split2 * avail).clamp(min_audio, (avail - top - min_bottom).max(min_audio));

        ui.allocate_ui(egui::vec2(width, top), |ui| {
            egui::ScrollArea::vertical().id_salt("section-top").auto_shrink([false, false]).show(ui, |ui| self.qso_top(ui));
        });
        let (dy, stopped) = Gui::divider(ui, width, grip);
        if dy != 0.0 {
            self.split = ((top + dy) / avail).clamp(0.05, 0.9);
        }

        ui.allocate_ui(egui::vec2(width, audio), |ui| {
            egui::ScrollArea::vertical().id_salt("section-audio").auto_shrink([false, false]).show(ui, |ui| self.qso_audio(ui));
        });
        let (dy2, stopped2) = Gui::divider(ui, width, grip);
        if dy2 != 0.0 {
            self.split2 = ((audio + dy2) / avail).clamp(0.05, 0.9);
        }
        if stopped || stopped2 {
            self.save_layout();
        }

        self.qso_lower(ui);
    }

    fn save_layout(&self) {
        Layout { top: self.split, audio: self.split2, gauges: self.gauge_order.clone() }.save();
    }

    /// A draggable divider. Returns how far it was dragged this frame, and whether the drag just ended.
    fn divider(ui: &mut egui::Ui, width: f32, height: f32) -> (f32, bool) {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        let c = if resp.hovered() || resp.dragged() { theme::accent() } else { theme::edge() };
        ui.painter().line_segment([egui::pos2(rect.left(), rect.center().y), egui::pos2(rect.right(), rect.center().y)], egui::Stroke::new(1.0, c.gamma_multiply(0.6)));
        for dx in [-8.0, 0.0, 8.0] {
            ui.painter().circle_filled(egui::pos2(rect.center().x + dx, rect.center().y), 1.8, c);
        }
        (if resp.dragged() { resp.drag_delta().y } else { 0.0 }, resp.drag_stopped())
    }

    /// Connect row, node entry, and the talk-and-station tile.
    fn qso_top(&mut self, ui: &mut egui::Ui) {
        self.deck(ui);
    }

    /// The controls you reach for most, kept at the very top: connect / disconnect (naming the
    /// station), dial a node, the echo test and recent stations, then the talk key and status.
    fn deck(&mut self, ui: &mut egui::Ui) {
        let selected = self.selected_station().cloned();
        let Some(s) = &mut self.session else { return };
        ui.add_space(6.0);
        let width = ui.available_width();
        let big = |text: String, color: egui::Color32| egui::Button::new(RichText::new(text).size(15.0).color(color).strong());
        match s.state.clone() {
            State::Connected { callsign, .. } => {
                if ui.add_sized([width, 34.0], big(format!("DISCONNECT    {callsign}"), theme::tx())).on_hover_text("Hang up").clicked() {
                    s.disconnect();
                }
            }
            State::Connecting { label, .. } => {
                if ui.add_sized([width, 34.0], big(format!("CANCEL    calling {label}…"), theme::accent())).on_hover_text("Stop calling").clicked() {
                    s.disconnect();
                }
            }
            State::Idle => match &selected {
                Some(st) => {
                    if ui.add_sized([width, 34.0], big(format!("CONNECT    {}", st.callsign), theme::accent())).on_hover_text("Call the station selected in the list").clicked() {
                        s.connect(st);
                    }
                }
                None => {
                    ui.add_enabled(false, egui::Button::new(RichText::new("NOT CONNECTED").size(15.0)).min_size(egui::vec2(width, 34.0)))
                        .on_disabled_hover_text("Pick a station in the list, or dial a node number below");
                }
            },
        }
        ui.add_space(6.0);
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
            let recent: Vec<String> = s.recent(10).iter().map(|c| c.call.clone()).collect();
            if !recent.is_empty() {
                let mut pick = None;
                egui::ComboBox::from_id_salt("recent").selected_text("RECENT").width(120.0).show_ui(ui, |ui| {
                    for call in &recent {
                        if ui.selectable_label(false, call).clicked() {
                            pick = Some(call.clone());
                        }
                    }
                });
                if let Some(call) = pick {
                    s.connect_call(&call);
                }
            }
        });


        ui.add_space(8.0);
        self.ptt_button(ui);
    }

    /// Gauges, mic and speaker controls, and the DTMF keypad.
    fn qso_audio(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        egui::CollapsingHeader::new(RichText::new("AUDIO").size(13.5).strong().color(theme::text())).default_open(true).show(ui, |ui| self.audio_panel(ui));
        self.keypad(ui);
    }

    /// Station log and chat, or the logbook.
    fn qso_lower(&mut self, ui: &mut egui::Ui) {
        let Some(s) = &self.session else { return };
        ui.add_space(4.0);
        Gui::tab_row(&mut self.tab, ui);
        if self.tab == Tab::Logbook {
            self.logbook_tab(ui);
            return;
        }
        if self.tab == Tab::Nets {
            self.nets_tab(ui);
            return;
        }
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

    fn hotkey_down_hint(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.cfg.ptt_hotkey.is_some()) && cfg!(target_os = "macos")
    }

    fn ptt_button(&mut self, ui: &mut egui::Ui) {
        let selected = self.selected_station().cloned();
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
            None if s.receiving => {
                let who = match &s.state {
                    State::Connected { callsign, .. } => callsign.clone(),
                    _ => String::new(),
                };
                let secs = s.rx_since.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                ("RECEIVING".to_string(), format!("{who}  ·  {}:{:02}", secs / 60, secs % 60), theme::rx(), false)
            }
            None if connected => ("PUSH TO TALK".to_string(), if self.hotkey_down_hint() { "hold here  ·  SPACE  ·  or your talk key" } else { "hold here  ·  or hold SPACE" }.to_string(), theme::accent(), false),
            None => ("NO LINK".to_string(), "connect to a station to talk".to_string(), theme::edge(), false),
        };
        // One tile in two halves: the talk key on the left, the station on the right.
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 88.0), egui::Sense::hover());
        let left_w = if rect.width() > 380.0 { (rect.width() * 0.58).clamp(230.0, rect.width() - 150.0) } else { rect.width() * 0.6 };
        let left = egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + left_w, rect.bottom()));
        let right = egui::Rect::from_min_max(egui::pos2(left.right(), rect.top()), rect.max);
        let sense = if connected { egui::Sense::click_and_drag() } else { egui::Sense::hover() };
        let r = ui.interact(left, ui.id().with("ptt-tile"), sense);
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
        p.rect_filled(rect, 4.0, theme::screen());
        p.rect_filled(left, egui::CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 }, fill);
        p.rect_stroke(rect, 4.0, egui::Stroke::new(if hot || connected { 2.0 } else { 1.0 }, accent), egui::StrokeKind::Inside);
        p.line_segment([egui::pos2(left.right(), rect.top() + 8.0), egui::pos2(left.right(), rect.bottom() - 8.0)], egui::Stroke::new(1.0, theme::edge()));
        // Corner screws, because it's hardware.
        for (dx, dy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            let c = egui::pos2(
                if dx > 0.0 { rect.left() + 9.0 } else { rect.right() - 9.0 },
                if dy > 0.0 { rect.top() + 9.0 } else { rect.bottom() - 9.0 },
            );
            p.circle_filled(c, 2.5, accent.gamma_multiply(0.35));
        }
        // Left half: lamp, status, and what to do.
        let lamp = egui::pos2(left.left() + 32.0, left.center().y);
        p.circle_filled(lamp, 15.0, accent.gamma_multiply(if hot { 0.25 * pulse + 0.1 } else { 0.1 }));
        p.circle_filled(lamp, 8.0, if hot || s.receiving || connected { accent } else { accent.gamma_multiply(0.4) });
        let tx = left.left() + 58.0;
        let title = ((left_w - 66.0) / 7.2).clamp(17.0, 30.0);
        let lp = p.with_clip_rect(left.shrink2(egui::vec2(4.0, 0.0)));
        lp.text(egui::pos2(tx, left.center().y - 11.0), egui::Align2::LEFT_CENTER, text, egui::FontId::new(title, theme::display()), if connected { accent } else { theme::muted() });
        lp.text(egui::pos2(tx, left.center().y + 20.0), egui::Align2::LEFT_CENTER, sub, egui::FontId::new(if left_w < 280.0 { 11.0 } else { 12.5 }, FontFamily::Monospace), theme::muted());

        // Right half: the station.
        let (label, call, detail, call_color) = match &s.state {
            State::Connected { callsign, name, .. } => {
                let info = s.remote_info.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or(name.trim()).to_string();
                ("LINKED", callsign.clone(), info, theme::accent())
            }
            State::Connecting { label, .. } => ("CALLING", label.clone(), String::new(), theme::accent()),
            State::Idle => match &selected {
                Some(st) => ("SELECTED", st.callsign.clone(), format!("NODE {}  {}", st.id, st.description), theme::accent()),
                None => ("STANDBY", "NOT CONNECTED".to_string(), "pick a station or dial a node".to_string(), theme::accent_dim()),
            },
        };
        let size = if call.len() > 10 { 17.0 } else if call.len() > 7 { 22.0 } else { 28.0 };
        let rp = p.with_clip_rect(right.shrink2(egui::vec2(8.0, 4.0)));
        let x = right.left() + 14.0;
        rp.text(egui::pos2(x, right.top() + 16.0), egui::Align2::LEFT_CENTER, label, egui::FontId::new(10.5, theme::display()), theme::muted());
        rp.text(egui::pos2(x, right.center().y + 1.0), egui::Align2::LEFT_CENTER, call, egui::FontId::new(size, theme::display()), call_color);
        rp.text(egui::pos2(x, right.bottom() - 15.0), egui::Align2::LEFT_CENTER, detail, egui::FontId::new(11.0, FontFamily::Monospace), theme::muted());
        self.ptt_held_by_mouse = down;
    }

    fn handle_ptt(&mut self, ctx: &egui::Context) {
        let Some(s) = &mut self.session else { return };
        // Space works while no text box has the keyboard.
        let typing = ctx.memory(|m| m.focused().is_some()) && ctx.wants_keyboard_input();
        let space = !typing && self.settings.is_none() && ctx.input(|i| i.key_down(egui::Key::Space));
        let hotkey = self.hotkey_down.load(Ordering::Relaxed);
        let want = (self.ptt_held_by_mouse || space || hotkey) && s.connected();
        if want != self.ptt_on {
            self.ptt_on = want;
            s.ptt(want);
        }
    }
}
