//! The settings window: station details, password and proxy.

use eframe::egui;
use echolink::config::{self, Config};

#[derive(PartialEq, Clone, Copy)]
enum ProxyMode {
    Direct,
    Auto,
    Custom,
}

/// Key combinations offered for the system-wide push-to-talk key (macOS).
const HOTKEYS: [(&str, &str); 5] = [
    ("Ctrl + Option + Space", "ctrl+alt+space"),
    ("Ctrl + Option + T", "ctrl+alt+t"),
    ("F13", "f13"),
    ("F14", "f14"),
    ("F15", "f15"),
];

pub struct Form {
    qrz_user: String,
    /// Blank keeps the saved password.
    qrz_pass: String,
    login_test: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    login_msg: Option<Result<String, String>>,
    qrz_key: String,
    blocked_text: String,
    allowed_text: String,
    /// The answer to "Test key", when it arrives.
    qrz_test: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    qrz_msg: Option<Result<String, String>>,
    inputs: Vec<String>,
    outputs: Vec<String>,
    cfg: Config,
    /// Blank keeps the saved password.
    password: String,
    proxy_mode: ProxyMode,
    proxy_host: String,
    error: String,
    first_run: bool,
}

pub enum Outcome {
    Open,
    Cancelled,
    Saved(Config),
}

impl Form {
    pub fn new(cfg: Option<&Config>) -> Form {
        let first_run = cfg.is_none();
        let cfg = cfg.cloned().unwrap_or_else(Config::blank);
        let (proxy_mode, proxy_host) = match cfg.proxy.as_deref().map(str::trim) {
            None | Some("") => (ProxyMode::Direct, String::new()),
            Some(p) if p.eq_ignore_ascii_case("none") => (ProxyMode::Direct, String::new()),
            Some(p) if p.eq_ignore_ascii_case("auto") => (ProxyMode::Auto, String::new()),
            Some(p) => (ProxyMode::Custom, p.to_string()),
        };
        let (inputs, outputs) = echolink::audio::devices();
        let qrz_user = cfg.qrz_user.clone().unwrap_or_default();
        let qrz_key = cfg.qrz_key.clone().unwrap_or_default();
        let blocked_text = cfg.blocked.join("\n");
        let allowed_text = cfg.allowed.join("\n");
        Form { qrz_user, qrz_pass: String::new(), login_test: None, login_msg: None, qrz_key, blocked_text, allowed_text, qrz_test: None, qrz_msg: None, inputs, outputs, cfg, password: String::new(), proxy_mode, proxy_host, error: String::new(), first_run }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let mut outcome = Outcome::Open;
        let title = if self.first_run { "Welcome to EchoLink" } else { "Settings" };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_width(440.0);
                if let Some(rx) = &self.login_test
                    && let Ok(answer) = rx.try_recv()
                {
                    self.login_msg = Some(answer);
                    self.login_test = None;
                }
                if let Some(rx) = &self.qrz_test
                    && let Ok(answer) = rx.try_recv()
                {
                    self.qrz_msg = Some(answer);
                    self.qrz_test = None;
                }
                egui::ScrollArea::vertical().max_height((ctx.screen_rect().height() - 190.0).max(260.0)).auto_shrink([false, true]).show(ui, |ui| {
                if self.first_run {
                    ui.label("Enter your station details. Your callsign must be validated for EchoLink at echolink.org.");
                    ui.add_space(6.0);
                }
                egui::Grid::new("settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label("Callsign");
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.callsign).hint_text("N0CALL").desired_width(200.0));
                    ui.end_row();

                    ui.label("Password");
                    let hint = if self.first_run { "EchoLink password" } else { "leave blank to keep" };
                    ui.add(egui::TextEdit::singleline(&mut self.password).password(true).hint_text(hint).desired_width(200.0));
                    ui.end_row();

                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.name).desired_width(200.0));
                    ui.end_row();

                    ui.label("Location");
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.location).hint_text("City, ST").char_limit(27).desired_width(200.0));
                    ui.end_row();

                    ui.label("Connection");
                    ui.vertical(|ui| {
                        ui.radio_value(&mut self.proxy_mode, ProxyMode::Direct, "Direct (UDP 5198–5199 forwarded to this computer)");
                        ui.radio_value(&mut self.proxy_mode, ProxyMode::Auto, "Public proxy, picked automatically");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.proxy_mode, ProxyMode::Custom, "This proxy:");
                            ui.add_enabled(
                                self.proxy_mode == ProxyMode::Custom,
                                egui::TextEdit::singleline(&mut self.proxy_host).hint_text("host[:port]").desired_width(150.0),
                            );
                        });
                        ui.label(
                            egui::RichText::new("Starlink, cellular and other shared-IP internet need a proxy.").small().weak(),
                        );
                    });
                    ui.end_row();

                    ui.label("");
                    ui.checkbox(&mut self.cfg.accept_incoming, "Accept calls from other stations");
                    ui.end_row();

                    ui.label("Microphone");
                    device_picker(ui, "mic", &self.inputs, &mut self.cfg.input_device);
                    ui.end_row();

                    ui.label("Speaker");
                    device_picker(ui, "speaker", &self.outputs, &mut self.cfg.output_device);
                    ui.end_row();

                    if cfg!(target_os = "macos") {
                        ui.label("Talk key");
                        ui.vertical(|ui| {
                            let shown = HOTKEYS
                                .iter()
                                .find(|(_, v)| self.cfg.ptt_hotkey.as_deref() == Some(*v))
                                .map(|(l, _)| *l)
                                .unwrap_or("Off");
                            egui::ComboBox::from_id_salt("hotkey").selected_text(shown).width(200.0).show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.cfg.ptt_hotkey, None, "Off");
                                for (label, value) in HOTKEYS {
                                    ui.selectable_value(&mut self.cfg.ptt_hotkey, Some(value.to_string()), label);
                                }
                            });
                            ui.label(egui::RichText::new("Hold it to talk, even when EchoLink is in the background.").small().weak());
                        });
                        ui.end_row();
                    }
                });

                ui.add_space(8.0);
                ui.separator();
                ui.label(egui::RichText::new("QRZ account").strong());
                ui.label(
                    egui::RichText::new(
                        "Log in to QRZ.com to see anyone's QRZ page inside EchoLink: click a callsign in the logbook, or type one in its search box. QRZ shows full details to XML subscribers.",
                    )
                    .small()
                    .weak(),
                );
                egui::Grid::new("qrzacct").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label("Username");
                    let hint = if self.cfg.callsign.is_empty() { "QRZ username".to_string() } else { self.cfg.callsign.clone() };
                    ui.add(egui::TextEdit::singleline(&mut self.qrz_user).hint_text(hint).desired_width(200.0));
                    ui.end_row();

                    ui.label("Password");
                    let hint = if self.cfg.qrz_pass.is_some() { "leave blank to keep" } else { "QRZ password" };
                    ui.add(egui::TextEdit::singleline(&mut self.qrz_pass).password(true).hint_text(hint).desired_width(200.0));
                    ui.end_row();

                    ui.label("");
                    ui.vertical(|ui| {
                        let busy = self.login_test.is_some();
                        let have_pass = !self.qrz_pass.is_empty() || self.cfg.qrz_pass.is_some();
                        if ui.add_enabled(!busy && have_pass && !self.qrz_user.trim().is_empty(), egui::Button::new(if busy { "Signing in…" } else { "Test login" }).small()).clicked() {
                            let (tx, rx) = std::sync::mpsc::channel();
                            let pass = if self.qrz_pass.is_empty() { self.cfg.qrz_pass.clone().unwrap_or_default() } else { self.qrz_pass.clone() };
                            let (user, ctx) = (self.qrz_user.clone(), ctx.clone());
                            std::thread::spawn(move || {
                                let client = echolink::lookup::Client::new(&user, &pass, Default::default());
                                let _ = tx.send(client.login().map(|s| {
                                    if s.subscription.is_empty() || s.subscription.to_lowercase().contains("non-subscriber") {
                                        "signed in, but no QRZ XML subscription: details will be limited".to_string()
                                    } else {
                                        format!("signed in, subscription until {}", s.subscription)
                                    }
                                }));
                                ctx.request_repaint();
                            });
                            self.login_test = Some(rx);
                            self.login_msg = None;
                        }
                        match &self.login_msg {
                            Some(Ok(m)) => drop(ui.colored_label(egui::Color32::from_rgb(90, 190, 110), m.clone())),
                            Some(Err(why)) => drop(ui.colored_label(ui.visuals().error_fg_color, format!("Not accepted: {why}"))),
                            None => {}
                        }
                    });
                    ui.end_row();
                });

                ui.add_space(8.0);
                ui.separator();
                ui.label(egui::RichText::new("QRZ logbook (upload and sync)").strong());
                ui.label(
                    egui::RichText::new(
                        "Every contact is saved on this Mac. Add your QRZ logbook key to also send each one to QRZ as soon as it ends (QRZ needs a subscription for uploads).",
                    )
                    .small()
                    .weak(),
                );
                egui::Grid::new("qrz").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label("API key");
                    ui.vertical(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.qrz_key).password(true).hint_text("XXXX-XXXX-XXXX-XXXX").desired_width(260.0));
                        ui.horizontal(|ui| {
                            if ui.small_button("Get my key…").on_hover_text("Opens your QRZ logbook. Open its settings, copy the API Key and paste it here.").clicked() {
                                let _ = std::process::Command::new("open").arg(echolink::qrz::LOGBOOK_URL).spawn();
                            }
                            let busy = self.qrz_test.is_some();
                            if ui.add_enabled(!busy && !self.qrz_key.trim().is_empty(), egui::Button::new(if busy { "Testing…" } else { "Test key" }).small()).clicked() {
                                let (tx, rx) = std::sync::mpsc::channel();
                                let (key, ctx) = (self.qrz_key.clone(), ctx.clone());
                                std::thread::spawn(move || {
                                    let _ = tx.send(echolink::qrz::Client::default().status(&key));
                                    ctx.request_repaint();
                                });
                                self.qrz_test = Some(rx);
                                self.qrz_msg = None;
                            }
                        });
                        match &self.qrz_msg {
                            Some(Ok(who)) => drop(ui.colored_label(egui::Color32::from_rgb(90, 190, 110), format!("Key works: {who}"))),
                            Some(Err(why)) => drop(ui.colored_label(ui.visuals().error_fg_color, format!("Not accepted: {why}"))),
                            None => {}
                        }
                    });
                    ui.end_row();

                    ui.label("");
                    ui.vertical(|ui| {
                        ui.checkbox(&mut self.cfg.qrz_auto, "Send each contact to QRZ as soon as it ends");
                        ui.checkbox(&mut self.cfg.qrz_on_exit, "Send any unsent contacts to QRZ when I quit");
                        ui.label(egui::RichText::new("Quitting waits a few seconds for QRZ to answer.").small().weak());
                    });
                    ui.end_row();

                    ui.label("Logged as");
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label("Band");
                            ui.add(egui::TextEdit::singleline(&mut self.cfg.log_band).desired_width(50.0));
                            ui.label("Mode");
                            ui.add(egui::TextEdit::singleline(&mut self.cfg.log_mode).desired_width(60.0));
                        });
                        ui.label(
                            egui::RichText::new("QRZ and ADIF need a band and mode. Contacts are marked EchoLink (PROP_MODE ECH) so they don't count for awards.")
                                .small()
                                .weak(),
                        );
                    });
                    ui.end_row();
                });

                ui.add_space(8.0);
                ui.separator();
                ui.label(egui::RichText::new("Who can call you").strong());
                egui::Grid::new("callcontrol").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label("Block list");
                    ui.vertical(|ui| {
                        ui.add(egui::TextEdit::multiline(&mut self.blocked_text).desired_rows(2).desired_width(260.0).hint_text("K6ABC, W1*"));
                        ui.label(egui::RichText::new("Calls or prefixes (end with *). One per line or comma separated.").small().weak());
                    });
                    ui.end_row();

                    ui.label("");
                    ui.checkbox(&mut self.cfg.allow_only, "Only accept calls from this list");
                    ui.end_row();

                    ui.label("Allow list");
                    ui.add_enabled(
                        self.cfg.allow_only,
                        egui::TextEdit::multiline(&mut self.allowed_text).desired_rows(2).desired_width(260.0).hint_text("K6ABC, K6*"),
                    );
                    ui.end_row();
                });
                });

                if !self.error.is_empty() {
                    ui.colored_label(ui.visuals().error_fg_color, &self.error);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(if self.first_run { "Start" } else { "Save" }).clicked() {
                        match self.validated() {
                            Ok(cfg) => match config::save(&cfg) {
                                Ok(()) => outcome = Outcome::Saved(cfg),
                                Err(e) => self.error = format!("Couldn't save: {e:#}"),
                            },
                            Err(e) => self.error = e,
                        }
                    }
                    if !self.first_run && ui.button("Cancel").clicked() {
                        outcome = Outcome::Cancelled;
                    }
                });
            });
        outcome
    }

    fn validated(&self) -> Result<Config, String> {
        let mut cfg = self.cfg.clone();
        cfg.callsign = cfg.callsign.trim().to_uppercase();
        cfg.location = cfg.location.trim().chars().take(27).collect();
        let key = self.qrz_key.trim();
        cfg.qrz_key = (!key.is_empty()).then(|| key.to_string());
        let user = self.qrz_user.trim();
        cfg.qrz_user = (!user.is_empty()).then(|| user.to_string());
        if !self.qrz_pass.is_empty() {
            cfg.qrz_pass = Some(self.qrz_pass.clone());
        }
        if cfg.qrz_user.is_none() {
            cfg.qrz_pass = None;
        }
        cfg.log_band = Some(cfg.log_band.trim().to_string()).filter(|b| !b.is_empty()).unwrap_or_else(|| "2m".into());
        cfg.log_mode = Some(cfg.log_mode.trim().to_uppercase()).filter(|m| !m.is_empty()).unwrap_or_else(|| "FM".into());
        cfg.blocked = config::parse_list(&self.blocked_text);
        cfg.allowed = config::parse_list(&self.allowed_text);
        if cfg.callsign.is_empty() {
            return Err("Enter your callsign.".into());
        }
        if !self.password.is_empty() {
            cfg.password = self.password.clone();
        }
        if cfg.password.is_empty() {
            return Err("Enter your EchoLink password.".into());
        }
        cfg.proxy = match self.proxy_mode {
            ProxyMode::Direct => None,
            ProxyMode::Auto => Some("auto".into()),
            ProxyMode::Custom if self.proxy_host.trim().is_empty() => return Err("Enter the proxy's host name.".into()),
            ProxyMode::Custom => Some(self.proxy_host.trim().into()),
        };
        Ok(cfg)
    }
}

/// A drop-down of sound devices. "System default" stores nothing, so the app
/// follows whatever macOS has selected.
fn device_picker(ui: &mut egui::Ui, id: &str, devices: &[String], choice: &mut Option<String>) {
    let shown = match choice {
        None => "System default".to_string(),
        Some(n) if devices.contains(n) => n.clone(),
        Some(n) => format!("{n} (not connected)"),
    };
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(260.0).show_ui(ui, |ui| {
        ui.selectable_value(choice, None, "System default");
        for d in devices {
            ui.selectable_value(choice, Some(d.clone()), d);
        }
    });
}
