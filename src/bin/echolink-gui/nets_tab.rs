//! The NETS tab: EchoLink nets from NetFinder, searchable, with each net's schedule in your
//! time and a button to join it.

use super::*;
use echolink::nets::{self, Detail, Net};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::Arc;

enum Msg {
    Progress(usize, usize),
    Done(Vec<Net>),
    Failed(String),
}

/// The one-time read of every net's page, to get their times.
enum TimesMsg {
    One(String, Detail),
    /// Stopped early, with why (empty if cancelled).
    Stopped(String),
    Done,
}

struct TimesJob {
    rx: Receiver<TimesMsg>,
    cancel: Arc<AtomicBool>,
    done: usize,
    total: usize,
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
    visible_for: Option<(String, String, bool, usize, usize, bool, bool)>,
    offset_min: Option<i32>,
    /// Every net's page that has been read, with when. Kept on disk for a week.
    times: HashMap<String, (i64, Detail)>,
    job: Option<TimesJob>,
    times_note: Option<String>,
    sort_soonest: bool,
}

impl NetsUi {
    /// Nets whose page has been read recently enough to trust.
    fn fresh(&self, slug: &str, now: i64) -> Option<&Detail> {
        self.times.get(slug).filter(|(at, _)| now - at < nets::DETAIL_MAX_AGE_SECS).map(|(_, d)| d)
    }

    fn known(&self, now: i64) -> usize {
        self.nets.iter().filter(|n| self.fresh(&n.slug, now).is_some()).count()
    }

    /// Times for nearly every net: show everything in your own time zone.
    fn local_mode(&self, now: i64) -> bool {
        !self.nets.is_empty() && self.known(now) * 10 >= self.nets.len() * 9
    }
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
            self.nets.times = nets::load_details();
            let now = echolink::contacts::now();
            let saved: Vec<(String, Detail)> = self.nets.times.iter().filter(|(_, (at, _))| now - at < nets::DETAIL_MAX_AGE_SECS).map(|(k, (_, d))| (k.clone(), d.clone())).collect();
            for (slug, d) in saved {
                self.nets.details.insert(slug, Ok(d)); // so a net opens at once
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

    /// Read every net's page, once, a second apart, so days and times can be shown in your time zone.
    fn nets_load_times(&mut self, ctx: &egui::Context) {
        let now = echolink::contacts::now();
        let todo: Vec<String> = self.nets.nets.iter().filter(|n| self.nets.fresh(&n.slug, now).is_none()).map(|n| n.slug.clone()).collect();
        if todo.is_empty() || self.nets.job.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.nets.job = Some(TimesJob { rx, cancel: cancel.clone(), done: 0, total: todo.len() });
        self.nets.times_note = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut failures = 0;
            for (i, slug) in todo.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(TimesMsg::Stopped(String::new()));
                    return;
                }
                if i > 0 {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                match nets::fetch_detail(nets::SITE, slug) {
                    Ok(d) => {
                        failures = 0;
                        let _ = tx.send(TimesMsg::One(slug.clone(), d));
                    }
                    Err(e) => {
                        failures += 1;
                        if failures >= 3 {
                            let _ = tx.send(TimesMsg::Stopped(e));
                            ctx.request_repaint();
                            return;
                        }
                    }
                }
                ctx.request_repaint();
            }
            let _ = tx.send(TimesMsg::Done);
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
        if let Some(job) = &mut n.job {
            let mut end = None;
            while let Ok(m) = job.rx.try_recv() {
                match m {
                    TimesMsg::One(slug, d) => {
                        job.done += 1;
                        n.details.insert(slug.clone(), Ok(d.clone()));
                        n.times.insert(slug, (echolink::contacts::now(), d));
                        if job.done % 20 == 0 {
                            nets::save_details(&n.times);
                        }
                    }
                    TimesMsg::Stopped(why) => end = Some(why),
                    TimesMsg::Done => end = Some(String::new()),
                }
            }
            if let Some(why) = end {
                nets::save_details(&n.times);
                n.times_note = (!why.is_empty()).then(|| format!("Stopped: {why}. What was read is kept; press the button to carry on."));
                n.job = None;
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
        let offset = *self.nets.offset_min.get_or_insert_with(nets::local_offset_minutes);
        let utc_today = nets::weekday(now);
        let my_today = nets::weekday(now + i64::from(offset) * 60);
        let local = self.nets.local_mode(now);
        // The day that counts as "today": yours once times are known, otherwise UTC's.
        let today = if local { my_today } else { utc_today };
        let loading = self.nets.rx.is_some();
        let can_refresh = !loading && self.nets.fetched_at.is_none_or(|t| now - t >= nets::MIN_REFRESH_SECS);

        let (mut refresh, mut load_times, mut cancel_times) = (false, false, false);
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
            if local {
                ui.checkbox(&mut self.nets.today_only, format!("On today ({})", &nets::DAYS[today][..3])).on_hover_text("Nets that meet on today's date where you are");
                ui.checkbox(&mut self.nets.sort_soonest, "Soonest first").on_hover_text("Order by which net starts next");
            } else {
                ui.checkbox(&mut self.nets.today_only, format!("On UTC {}", &nets::DAYS[today][..3]))
                    .on_hover_text("Only nets that meet on the current UTC day. NetFinder lists days in UTC, so a net can be on tomorrow's UTC day while it is still evening for you.");
            }
        });

        // Days in your time zone need each net's time, which is on its own page.
        if let Some(job) = &self.nets.job {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("Reading net times… {} of {}", job.done, job.total)).small().color(theme::accent()));
                if ui.small_button("CANCEL").clicked() {
                    cancel_times = true;
                }
            });
        } else if local {
            ui.label(RichText::new("Days and times are in your time zone.").small().color(theme::muted()));
        } else {
            let todo = self.nets.nets.len().saturating_sub(self.nets.known(now));
            ui.horizontal_wrapped(|ui| {
                let days_note = if my_today == utc_today {
                    format!("Days are in UTC (it's {} here and in UTC).", nets::DAYS[utc_today])
                } else {
                    format!("Days are in UTC: it's {} where you are, but {} in UTC.", nets::DAYS[my_today], nets::DAYS[utc_today])
                };
                ui.label(RichText::new(days_note).small().color(theme::muted()));
                if todo > 0
                    && ui
                        .small_button("SHOW IN MY TIME ZONE")
                        .on_hover_text(format!(
                            "Needs each net's time, which is only on its own page: {todo} page requests to NetFinder, one per second (about {} minutes), once. The times are kept for a week.",
                            todo.div_ceil(60)
                        ))
                        .clicked()
                {
                    load_times = true;
                }
            });
        }
        if let Some(note) = &self.nets.times_note {
            ui.label(RichText::new(note).small().color(theme::tx()));
        }

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

        // Which nets pass the filters, and in what order. A net's days are yours when its page has been read.
        let days_of = |ui_state: &NetsUi, n: &Net| -> ([bool; 7], bool) {
            if local && let Some(d) = ui_state.fresh(&n.slug, now) && let Some(days) = nets::local_days(d, offset) {
                return (days, true);
            }
            (n.days, false)
        };
        let key = (self.nets.search.trim().to_lowercase(), self.nets.band.clone(), self.nets.today_only, self.nets.nets.len(), today, local, self.nets.sort_soonest);
        if self.nets.visible_for.as_ref() != Some(&key) {
            let mut idx: Vec<usize> = self
                .nets
                .nets
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    (key.0.is_empty() || n.name.to_lowercase().contains(&key.0)) && (key.1.is_empty() || n.band == key.1) && (!key.2 || days_of(&self.nets, n).0[today])
                })
                .map(|(i, _)| i)
                .collect();
            if local && self.nets.sort_soonest {
                let soon = |i: &usize| self.nets.fresh(&self.nets.nets[*i].slug, now).and_then(|d| nets::minutes_until_next(d, now)).unwrap_or(i32::MAX);
                idx.sort_by_key(soon);
            }
            self.nets.visible = idx;
            self.nets.visible_for = Some(key);
        }
        ui.label(RichText::new(format!("{} shown", self.nets.visible.len())).small().color(theme::muted()));

        let compact = ui.available_width() < 560.0;
        let table_height = ui.available_height().max(100.0);
        let mut picked: Option<String> = None;
        let selected = self.nets.selected.clone();
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(120.0).clip(true))
            .column(Column::exact(94.0));
        if local && !compact {
            table = table.column(Column::exact(96.0));
        }
        table = table.column(Column::exact(74.0));
        if !compact {
            table = table.column(Column::initial(130.0).clip(true));
        }
        table
            .max_scroll_height(table_height)
            .header(22.0, |mut h| {
                let days_head = if local { "Days (you)" } else { "Days (UTC)" };
                let mut heads = vec!["Net", days_head];
                if local && !compact {
                    heads.push("Starts");
                }
                heads.push("Band");
                if !compact {
                    heads.push("Type");
                }
                for t in heads {
                    h.col(|ui| {
                        ui.label(theme::silk(t));
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, self.nets.visible.len(), |mut row| {
                    let n = &self.nets.nets[self.nets.visible[row.index()]];
                    let (days, exact) = days_of(&self.nets, n);
                    row.set_selected(selected.as_deref() == Some(n.slug.as_str()));
                    row.col(|ui| {
                        ui.label(RichText::new(&n.name).size(13.0));
                    });
                    row.col(|ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            for (i, letter) in "MTWTFSS".chars().enumerate() {
                                // Faded when these are UTC days because the net's time isn't known yet.
                                let fade = if local && !exact { 0.6 } else { 1.0 };
                                let color = match (days[i], i == today) {
                                    (true, true) => theme::rx(),
                                    (true, false) => theme::accent().gamma_multiply(fade),
                                    _ => theme::muted().gamma_multiply(0.4),
                                };
                                let mut text = RichText::new(letter.to_string()).monospace().size(12.0).color(color);
                                if !local && i == my_today && my_today != utc_today {
                                    text = text.underline(); // your own day, when it differs from UTC's
                                }
                                let basis = if local && exact { "your time" } else { "UTC" };
                                let tip = format!("{} ({basis}){}", nets::DAYS[i], if days[i] { ": meets" } else { ": no net" });
                                ui.label(text).on_hover_text(tip);
                            }
                        });
                    });
                    if local && !compact {
                        row.col(|ui| {
                            let start = self.nets.fresh(&n.slug, now).and_then(|d| nets::local_start(d, offset)).unwrap_or_else(|| "-".into());
                            ui.label(RichText::new(start).monospace().size(12.0));
                        });
                    }
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
        if load_times {
            self.nets_load_times(&ctx);
        }
        if cancel_times && let Some(job) = &self.nets.job {
            job.cancel.store(true, Ordering::Relaxed);
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
