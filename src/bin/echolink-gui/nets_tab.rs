//! The NETS tab: EchoLink nets from NetFinder, searchable, with each net's schedule in your
//! time and a button to join it.

use super::*;
use echolink::nets::{self, Detail, Net};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};

enum Msg {
    Progress(usize, usize),
    Done(Vec<Net>),
    Failed(String),
}

#[derive(Default)]
pub struct NetsUi {
    nets: Vec<Net>,
    fetched_at: Option<i64>,
    rx: Option<Receiver<Msg>>,
    progress: Option<(usize, usize)>,
    error: Option<String>,
    started: bool,
    selected: Option<String>,
    details: HashMap<String, Result<Detail, String>>,
    pending: Vec<(String, Receiver<Result<Detail, String>>)>,
    search: String,
    /// Empty means every band.
    band: String,
    today_only: bool,
    /// The nets that pass the filters, as indexes into `nets`, and what that was worked out for.
    visible: Vec<usize>,
    visible_for: Option<(String, String, bool, usize, usize)>,
    offset_min: Option<i32>,
}

impl Gui {
    pub(super) fn nets_tab(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.nets_poll();
        if !self.nets.started {
            self.nets.started = true;
            if let Some((at, list)) = nets::load_cache() {
                self.nets.nets = list;
                self.nets.fetched_at = Some(at);
            }
            let stale = self.nets.fetched_at.is_none_or(|t| echolink::contacts::now() - t > nets::MAX_AGE_SECS);
            if stale {
                self.nets_fetch(&ctx);
            }
        }

        let height = ui.available_height().max(120.0);
        let width = ui.available_width();
        ui.allocate_ui(egui::vec2(width, height), |ui| {
            if self.nets.selected.is_some() {
                let mut close = false;
                egui::SidePanel::right("net-pane")
                    .resizable(true)
                    .default_width((ui.available_width() * 0.5).clamp(280.0, 440.0))
                    .min_width(240.0)
                    .max_width((ui.available_width() * 0.75).max(260.0))
                    .frame(egui::Frame::new().fill(theme::screen()).stroke(egui::Stroke::new(1.0, theme::edge())).corner_radius(4).inner_margin(egui::Margin::symmetric(10, 8)))
                    .show_inside(ui, |ui| close = self.nets_detail(ui));
                if close {
                    self.nets.selected = None;
                }
            }
            egui::CentralPanel::default().frame(egui::Frame::new()).show_inside(ui, |ui| self.nets_list(ui));
        });
    }

    fn nets_fetch(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        self.nets.rx = Some(rx);
        self.nets.progress = Some((0, 0));
        self.nets.error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = nets::fetch_list(nets::SITE, |done, total| {
                let _ = tx.send(Msg::Progress(done, total));
                ctx.request_repaint();
            });
            let _ = tx.send(match result {
                Ok(list) => Msg::Done(list),
                Err(e) => Msg::Failed(e),
            });
            ctx.request_repaint();
        });
    }

    fn nets_select(&mut self, ctx: &egui::Context, slug: &str) {
        self.nets.selected = Some(slug.to_string());
        if self.nets.details.contains_key(slug) || self.nets.pending.iter().any(|(s, _)| s == slug) {
            return;
        }
        let (tx, rx) = channel();
        self.nets.pending.push((slug.to_string(), rx));
        let (ctx, slug) = (ctx.clone(), slug.to_string());
        std::thread::spawn(move || {
            let _ = tx.send(nets::fetch_detail(nets::SITE, &slug));
            ctx.request_repaint();
        });
    }

    fn nets_poll(&mut self) {
        let n = &mut self.nets;
        if let Some(rx) = &n.rx {
            let mut finished = false;
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Progress(d, t) => n.progress = Some((d, t)),
                    Msg::Done(list) => {
                        let now = echolink::contacts::now();
                        nets::save_cache(now, &list);
                        n.nets = list;
                        n.fetched_at = Some(now);
                        n.progress = None;
                        finished = true;
                    }
                    Msg::Failed(e) => {
                        n.error = Some(e);
                        n.progress = None;
                        finished = true;
                    }
                }
            }
            if finished {
                n.rx = None;
            }
        }
        let mut still = Vec::new();
        for (slug, rx) in std::mem::take(&mut n.pending) {
            match rx.try_recv() {
                Ok(result) => {
                    n.details.insert(slug, result);
                }
                Err(_) => still.push((slug, rx)),
            }
        }
        n.pending = still;
    }

    /// The list: filters on top, nets below.
    fn nets_list(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = echolink::contacts::now();
        let today = nets::weekday(now);
        let loading = self.nets.rx.is_some();
        let can_refresh = !loading && self.nets.fetched_at.is_none_or(|t| now - t >= nets::MIN_REFRESH_SECS);

        let mut refresh = false;
        ui.horizontal_wrapped(|ui| {
            refresh = ui
                .add_enabled(can_refresh, egui::Button::new(if loading { "LOADING…" } else { "REFRESH" }))
                .on_hover_text("Read the net list again. It's kept for a day, so this is rarely needed.")
                .clicked();
            let r = ui.add(egui::TextEdit::singleline(&mut self.nets.search).hint_text("search nets").desired_width(150.0));
            if ui.add_enabled(!self.nets.search.is_empty(), egui::Button::new("CLR")).clicked() {
                self.nets.search.clear();
                r.request_focus();
            }
            let mut bands: Vec<&str> = self.nets.nets.iter().map(|n| n.band.as_str()).filter(|b| !b.is_empty()).collect();
            bands.sort();
            bands.dedup();
            let shown = if self.nets.band.is_empty() { "All bands".to_string() } else { self.nets.band.clone() };
            egui::ComboBox::from_id_salt("net-band").selected_text(shown).width(110.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.nets.band, String::new(), "All bands");
                for b in bands {
                    ui.selectable_value(&mut self.nets.band, b.to_string(), b);
                }
            });
            ui.checkbox(&mut self.nets.today_only, format!("On today ({})", &nets::DAYS[today][..3])).on_hover_text("Only nets that meet on today's UTC day");
        });

        let status = if let Some((d, t)) = self.nets.progress {
            if t == 0 { "Reading the net list from NetFinder…".to_string() } else { format!("Reading the net list from NetFinder… page {d} of {t}") }
        } else if let Some(e) = &self.nets.error {
            format!("Couldn't read the list: {e}")
        } else if let Some(at) = self.nets.fetched_at {
            format!("{} EchoLink nets · list read {} UTC", self.nets.nets.len(), echolink::contacts::utc_label(at))
        } else {
            "No net list yet.".to_string()
        };
        let color = if self.nets.error.is_some() { theme::tx() } else { theme::muted() };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(status).small().color(color));
            if ui.small_button("NetFinder.radio").on_hover_text("Open NetFinder in your browser").clicked() {
                let _ = std::process::Command::new("open").arg(nets::SITE).spawn();
            }
        });

        // Which nets pass the filters.
        let key = (self.nets.search.trim().to_lowercase(), self.nets.band.clone(), self.nets.today_only, self.nets.nets.len(), today);
        if self.nets.visible_for.as_ref() != Some(&key) {
            self.nets.visible = self
                .nets
                .nets
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    (key.0.is_empty() || n.name.to_lowercase().contains(&key.0))
                        && (key.1.is_empty() || n.band == key.1)
                        && (!key.2 || n.days[today])
                })
                .map(|(i, _)| i)
                .collect();
            self.nets.visible_for = Some(key);
        }
        ui.label(RichText::new(format!("{} shown", self.nets.visible.len())).small().color(theme::muted()));

        let compact = ui.available_width() < 520.0;
        let table_height = ui.available_height().max(100.0);
        let mut picked: Option<String> = None;
        let selected = self.nets.selected.clone();
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(120.0).clip(true))
            .column(Column::exact(94.0))
            .column(Column::exact(74.0));
        if !compact {
            table = table.column(Column::initial(150.0).clip(true));
        }
        table
            .max_scroll_height(table_height)
            .header(22.0, |mut h| {
                let heads: &[&str] = if compact { &["Net", "Days (UTC)", "Band"] } else { &["Net", "Days (UTC)", "Band", "Type"] };
                for t in heads {
                    h.col(|ui| {
                        ui.label(theme::silk(t));
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, self.nets.visible.len(), |mut row| {
                    let n = &self.nets.nets[self.nets.visible[row.index()]];
                    row.set_selected(selected.as_deref() == Some(n.slug.as_str()));
                    row.col(|ui| {
                        ui.label(RichText::new(&n.name).size(13.0));
                    });
                    row.col(|ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            for (i, letter) in "MTWTFSS".chars().enumerate() {
                                let color = match (n.days[i], i == today) {
                                    (true, true) => theme::rx(),
                                    (true, false) => theme::accent(),
                                    _ => theme::muted().gamma_multiply(0.4),
                                };
                                ui.label(RichText::new(letter.to_string()).monospace().size(12.0).color(color));
                            }
                        });
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&n.band).small().color(theme::muted()));
                    });
                    if !compact {
                        row.col(|ui| {
                            let tags: Vec<&str> = n.tags.iter().map(String::as_str).filter(|t| *t != "EchoLink").collect();
                            ui.label(RichText::new(tags.join(" · ")).small().color(theme::muted()));
                        });
                    }
                    if row.response().clicked() {
                        picked = Some(n.slug.clone());
                    }
                });
            });

        if refresh {
            self.nets_fetch(&ctx);
        }
        if let Some(slug) = picked {
            self.nets_select(&ctx, &slug);
        }
    }

    /// One net's details. Returns true when it should close.
    fn nets_detail(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(slug) = self.nets.selected.clone() else { return true };
        let mut close = false;
        let mut join: Option<String> = None;
        let offset = *self.nets.offset_min.get_or_insert_with(nets::local_offset_minutes);
        let listing = self.nets.nets.iter().find(|n| n.slug == slug).cloned();

        ui.horizontal(|ui| {
            ui.label(theme::silk("NET"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("CLOSE").clicked() {
                    close = true;
                }
            });
        });
        ui.separator();
        match self.nets.details.get(&slug) {
            None => {
                if let Some(n) = &listing {
                    ui.label(theme::head(&n.name, 18.0).color(theme::accent()));
                }
                ui.label(RichText::new("Asking NetFinder…").color(theme::muted()));
            }
            Some(Err(why)) => {
                if let Some(n) = &listing {
                    ui.label(theme::head(&n.name, 18.0).color(theme::accent()));
                }
                ui.colored_label(ui.visuals().error_fg_color, why);
            }
            Some(Ok(d)) => {
                let online = d.echolink_node.as_ref().and_then(|node| {
                    let s = self.session.as_ref()?;
                    let base = node.trim_end_matches("-R").trim_end_matches("-L");
                    [node.clone(), format!("{base}-R"), format!("{base}-L"), base.to_string()].into_iter().find(|c| s.find(c).is_some())
                });
                egui::ScrollArea::vertical().auto_shrink([false, false]).max_height((ui.available_height() - 40.0).max(100.0)).show(ui, |ui| {
                    ui.label(theme::head(if d.name.is_empty() { listing.as_ref().map(|n| n.name.as_str()).unwrap_or("") } else { &d.name }, 19.0).color(theme::accent()));
                    if let Some(n) = &listing {
                        ui.label(RichText::new(format!("{}  ·  {}", n.band, n.tags.join(" · "))).small().color(theme::muted()));
                    }
                    ui.add_space(6.0);
                    ui.label(theme::silk("SCHEDULE"));
                    if d.slots.is_empty() {
                        ui.label(RichText::new("No times listed.").color(theme::muted()));
                    }
                    for s in &d.slots {
                        egui::Frame::new().fill(theme::chassis()).corner_radius(3).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if let Some(mine) = nets::in_my_time(&s.utc, offset) {
                                ui.label(RichText::new(format!("Your time   {mine}")).monospace().size(13.0).color(theme::rx()));
                            }
                            ui.label(RichText::new(format!("UTC         {}", s.utc)).monospace().size(12.5));
                            ui.label(RichText::new(format!("Net's time  {} {}", s.local, s.timezone)).monospace().size(12.0).color(theme::muted()));
                            if !s.frequency.is_empty() {
                                ui.label(RichText::new(&s.frequency).size(12.5));
                            }
                        });
                    }
                    ui.add_space(6.0);
                    if let Some(node) = &d.echolink_node {
                        ui.label(theme::silk("ECHOLINK NODE"));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(node).monospace().size(15.0).color(theme::accent()));
                            let connect = ui.add_enabled(online.is_some(), egui::Button::new("JOIN NOW"));
                            if connect.on_hover_text("Connect to this node on EchoLink").on_disabled_hover_text("This node isn't online in the directory right now").clicked() {
                                join = online.clone();
                            }
                        });
                        if online.is_none() {
                            ui.label(RichText::new("Not online right now.").small().color(theme::muted()));
                        }
                    }
                    ui.add_space(6.0);
                    egui::Grid::new("net-fields").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                        for (label, value) in d.fields.iter().filter(|(l, _)| !l.eq_ignore_ascii_case("EchoLink Node") && !l.eq_ignore_ascii_case("Band")) {
                            ui.label(theme::silk(label));
                            if (label.eq_ignore_ascii_case("Website") || value.starts_with("http")) && (value.starts_with("https://") || value.starts_with("http://")) {
                                if ui.link(value).clicked() {
                                    let _ = std::process::Command::new("open").arg(value).spawn();
                                }
                            } else {
                                ui.label(RichText::new(value).size(12.5));
                            }
                            ui.end_row();
                        }
                    });
                    ui.add_space(8.0);
                    ui.label(RichText::new("From NetFinder (netfinder.radio), kept up to date by its users. Check the net's own page before you go.").small().color(theme::muted()));
                });
            }
        }
        ui.add_space(4.0);
        if let Some(url) = nets::page_url(&slug)
            && ui.button("OPEN ON NETFINDER").clicked()
        {
            let _ = std::process::Command::new("open").arg(url).spawn();
        }
        if let (Some(call), Some(s)) = (join, &mut self.session) {
            s.connect_call(&call);
        }
        close
    }
}
