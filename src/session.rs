//! A running EchoLink station, independent of any user interface: the
//! directory worker, the station list, call state, and the message log.
//! Front ends call `poll()` regularly and the action methods on input.

use crate::audio::{self, Audio};
use crate::config::{Config, Rules};
use crate::contacts::{self, Contact, Qrz};
use crate::logbook::{self, Entry, QrzQso, Worked};
use crate::directory::{self, Kind, Station, Status};
use crate::net::Net;
use crate::qso::{self, Cmd, Event, State};
use anyhow::Result;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

const RELOGIN: Duration = Duration::from_secs(5 * 60);
/// How often to check that the proxy connection is still up.
/// How soon to try again after a login that never got an answer.
const RETRY: Duration = Duration::from_secs(15);
const TICK: Duration = Duration::from_secs(10);
const LOG_LINES: usize = 500;

enum DirCmd {
    Status(Status),
    Refresh,
    Quit,
}

enum DirEvent {
    LoggedIn(Status, String),
    Stations(Vec<Station>),
    Error(String),
    Done,
}

/// Keeps us registered with the directory and fetches the station list.
/// With a proxy, a dropped connection is replaced and we log in again
/// through the new one (other stations see us at the proxy's IP).
fn directory_worker(cfg: Config, net: Net) -> (Sender<DirCmd>, Receiver<DirEvent>) {
    let (cmd_tx, cmd_rx) = channel();
    let (ev_tx, ev_rx) = channel();
    std::thread::spawn(move || {
        let mut status = Status::Online;
        let mut fetch = true;
        loop {
            let mut wait = RELOGIN;
            let result = directory::login(&net, &cfg.servers, &cfg.callsign, &cfg.password, &cfg.location, status);
            crate::log!("directory login as {} ({status:?}): {}", cfg.callsign, result.as_ref().map(|_| "OK".to_string()).unwrap_or_else(|e| format!("{e:#}")));
            match result {
                Ok(()) => {
                    let _ = ev_tx.send(DirEvent::LoggedIn(status, net.describe()));
                    if fetch {
                        match directory::stations(&net, &cfg.servers) {
                            Ok(s) => {
                                crate::log!("station list: {} stations", s.len());
                                if let Some(w) = directory::stub_warning(&s) {
                                    crate::log!("WARNING: {w}");
                                    let _ = ev_tx.send(DirEvent::Error(w));
                                }
                                let _ = ev_tx.send(DirEvent::Stations(s));
                            }
                            Err(e) => {
                                crate::log!("station list failed: {e:#}");
                                let _ = ev_tx.send(DirEvent::Error(format!("{e:#}")));
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = ev_tx.send(DirEvent::Error(format!("{e:#}")));
                    if e.downcast_ref::<directory::Refused>().is_none() {
                        // No answer at all: try another proxy, and soon.
                        if let Net::Proxy(m) = &net {
                            m.abandon("couldn't reach the directory");
                        }
                        wait = RETRY;
                    }
                }
            }
            // Keep asking for the list until we have one.
            fetch &= wait == RETRY;
            let next_login = Instant::now() + wait;
            loop {
                match cmd_rx.recv_timeout(TICK) {
                    Ok(DirCmd::Status(s)) => {
                        status = s;
                        break;
                    }
                    Ok(DirCmd::Refresh) => {
                        fetch = true;
                        break;
                    }
                    Ok(DirCmd::Quit) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        let _ = directory::login(&net, &cfg.servers, &cfg.callsign, &cfg.password, &cfg.location, Status::Off);
                        let _ = ev_tx.send(DirEvent::Done);
                        return;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
                if let Net::Proxy(m) = &net
                    && m.is_down()
                {
                    crate::log!("proxy connection is down; reconnecting");
                    let _ = ev_tx.send(DirEvent::Error("Proxy connection lost; reconnecting…".into()));
                    fetch = true;
                    break;
                }
                if Instant::now() >= next_login {
                    break;
                }
            }
        }
    });
    (cmd_tx, ev_rx)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Filter {
    All,
    Repeaters,
    Links,
    Conferences,
    Users,
}

impl Filter {
    pub const ALL: [Filter; 5] = [Filter::All, Filter::Repeaters, Filter::Links, Filter::Conferences, Filter::Users];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Repeaters => "Repeaters",
            Filter::Links => "Links",
            Filter::Conferences => "Conferences",
            Filter::Users => "Users",
        }
    }

    pub fn matches(self, k: Kind) -> bool {
        match self {
            Filter::All => true,
            Filter::Repeaters => k == Kind::Repeater,
            Filter::Links => k == Kind::Link,
            Filter::Conferences => k == Kind::Conference,
            Filter::Users => k == Kind::User,
        }
    }
}

/// What `Session::poll` found.
#[derive(PartialEq)]
pub enum Update {
    Nothing,
    StationsChanged,
    /// Logged off after `quit()`; safe to exit.
    Finished,
}

#[derive(Clone, Copy, PartialEq)]
pub enum LogKind {
    System,
    Chat,
}

/// Called from a background thread for incoming calls and chat from others,
/// so it works even while the front end isn't drawing (e.g. window hidden).
pub type Notifier = Box<dyn Fn(&str) + Send>;

pub struct Session {
    pub cfg: Config,
    pub audio: Audio,
    cmds: Sender<Cmd>,
    events: Receiver<Event>,
    transmitting: Arc<AtomicBool>,
    dir_cmds: Sender<DirCmd>,
    dir_events: Receiver<DirEvent>,

    /// Sorted by callsign.
    pub stations: Vec<Station>,
    pub dir_status: String,
    pub dir_ok: bool,
    pub state: State,
    pub remote_info: String,
    pub log: VecDeque<(LogKind, String)>,
    pub receiving: bool,
    pub rx_since: Option<Instant>,
    pub tx_since: Option<Instant>,
    pub quitting: bool,
    dtmf: Sender<String>,
    /// Who may call us; change it and the engine follows at once.
    pub rules: Arc<RwLock<Rules>>,
    /// Everything you've been connected to, oldest first.
    pub contacts: Vec<Contact>,
    active: Option<Active>,
    pending_incoming: Option<String>,
    /// Set when someone connects to us; the window takes it to get your attention.
    pub incoming: Option<String>,
    qrz_tx: Sender<UploadDone>,
    qrz_rx: Receiver<UploadDone>,
    /// QRZ log ids hidden in this app only.
    pub hidden: HashSet<u64>,
    /// Deletes asked of QRZ and not answered yet: log id -> the logbook entry to drop when QRZ agrees.
    pending_deletes: HashMap<u64, String>,
    del_tx: Sender<(u64, Result<(), String>)>,
    del_rx: Receiver<(u64, Result<(), String>)>,
    /// Your QRZ logbook, copied here by [`Session::sync_qrz`].
    pub qrz_book: Vec<QrzQso>,
    /// The whole logbook for display, newest first. `book_gen` changes when it does.
    pub book: Vec<Entry>,
    pub book_gen: u64,
    /// Who you've worked, by [`logbook::norm`] call.
    pub worked: HashMap<String, Worked>,
    pub sync: SyncState,
    pub last_sync: Option<i64>,
    sync_tx: Sender<SyncMsg>,
    sync_rx: Receiver<SyncMsg>,
    /// During a full re-sync: the ids QRZ still has, so deleted ones can go.
    sync_seen: Option<HashSet<u64>>,
    sync_new: usize,
    /// Uploads to QRZ that haven't answered yet; closing waits for them.
    uploads_in_flight: usize,
    /// The directory log-off is done; waiting only for uploads.
    dir_done: bool,
    exit_deadline: Option<Instant>,
}

/// The answer to one upload to QRZ.
struct UploadDone {
    id: String,
    status: Qrz,
    /// The old QRZ copy of an edited contact, now deleted from QRZ.
    replaced: Option<u64>,
    /// Something to tell the user.
    note: Option<String>,
}

/// An edit to a contact made in this app.
pub struct ContactEdit {
    pub call: String,
    pub name: String,
    pub start: i64,
    pub secs: u64,
    pub band: String,
    pub mode: String,
    pub echolink: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SyncState {
    Idle,
    /// Downloading; contacts received so far.
    Running(usize),
    /// Finished; contacts that were new, and the size of the copy now.
    Done { new: usize, total: usize },
    Failed(String),
}

enum SyncMsg {
    Page(Vec<QrzQso>),
    Done,
    Failed(String),
}

/// The connection in progress, to become a [`Contact`] when it ends.
struct Active {
    call: String,
    name: String,
    start: i64,
    since: Instant,
    incoming: bool,
}

/// Plays touch-tone digits into the transmission, one batch at a time. Keys up
/// by itself if you aren't already talking, and keeps the microphone out of it.
fn dtmf_worker(
    digits: Receiver<String>,
    cmds: Sender<Cmd>,
    shared: Arc<audio::Shared>,
    mic: Sender<Vec<i16>>,
    transmitting: Arc<AtomicBool>,
) {
    use std::time::Duration;
    std::thread::spawn(move || {
        while let Ok(first) = digits.recv() {
            let already_keyed = transmitting.load(Ordering::Relaxed);
            shared.mute_mic.store(true, Ordering::Relaxed);
            if !already_keyed {
                let _ = cmds.send(Cmd::Ptt(true));
                std::thread::sleep(Duration::from_millis(250)); // let the far end open up
            }
            let mut next = Some(first);
            while let Some(d) = next.take() {
                let start = Instant::now();
                for (i, chunk) in crate::dtmf::pcm(&d).chunks(160).enumerate() {
                    let _ = mic.send(chunk.to_vec());
                    // Real-time pace, 20 ms per chunk.
                    let due = start + Duration::from_millis(20 * (i as u64 + 1));
                    std::thread::sleep(due.saturating_duration_since(Instant::now()));
                }
                next = digits.try_recv().ok();
            }
            std::thread::sleep(Duration::from_millis(150));
            if !already_keyed {
                let _ = cmds.send(Cmd::Ptt(false));
            }
            shared.mute_mic.store(false, Ordering::Relaxed);
        }
    });
}

impl Session {
    /// Open the sound card, start the network engine, and log in.
    /// `client` names the program in the info text other stations see.
    pub fn start(cfg: Config, client: &str, notifier: Option<Notifier>) -> Result<Session> {
        crate::log!("{client} {} starting as {}", env!("CARGO_PKG_VERSION"), cfg.callsign);
        let transmitting = Arc::new(AtomicBool::new(false));
        let (mic_tx, mic_rx) = channel();
        let dtmf_mic = mic_tx.clone();
        let audio = audio::open(mic_tx, transmitting.clone(), cfg.input_device.as_deref(), cfg.output_device.as_deref())?;
        crate::log!("audio: {}", audio.description);
        audio.shared.set_volume(cfg.speaker_volume);
        audio.shared.set_gain_db(cfg.mic_gain_db);
        audio.shared.auto_gain.store(cfg.auto_gain, std::sync::atomic::Ordering::Relaxed);
        audio.shared.denoise.store(cfg.noise_reduction, std::sync::atomic::Ordering::Relaxed);
        let rules = Arc::new(RwLock::new(Rules::from_config(&cfg)));
        let me = qso::Station {
            callsign: cfg.callsign.clone(),
            name: cfg.name.clone(),
            info: format!("{}\n{}\n{}\n{client} {} (Linux)", cfg.callsign, cfg.name, cfg.location, env!("CARGO_PKG_VERSION")),
            accept_incoming: cfg.accept_incoming,
            rules: rules.clone(),
        };
        let (udp_tx, udp_rx) = channel();
        let net = Net::from_config(&cfg, udp_tx);
        if let Net::Proxy(_) = net {
            crate::log!("using an EchoLink proxy ({})", cfg.proxy.as_deref().unwrap_or(""));
        }
        let qso = qso::spawn(me, audio.shared.clone(), mic_rx, transmitting, &net, udp_rx)?;
        let (dir_cmds, dir_events) = directory_worker(cfg.clone(), net);
        let (ev_tx, events) = channel();
        let mine = format!("{}>", cfg.callsign);
        let qso_events = qso.events;
        std::thread::spawn(move || {
            for e in qso_events {
                if let Some(n) = &notifier {
                    match &e {
                        Event::Incoming(call) => n(&format!("Incoming EchoLink call from {call}")),
                        Event::Chat(t) if !t.starts_with(&mine) => n(t),
                        _ => {}
                    }
                }
                if ev_tx.send(e).is_err() {
                    return;
                }
            }
        });
        let (qrz_tx, qrz_rx) = channel();
        let (del_tx, del_rx) = channel();
        let (sync_tx, sync_rx) = channel();
        let (dtmf, dtmf_rx) = channel();
        dtmf_worker(dtmf_rx, qso.cmds.clone(), audio.shared.clone(), dtmf_mic, qso.transmitting.clone());
        let mut s = Session {
            cfg,
            audio,
            cmds: qso.cmds,
            events,
            transmitting: qso.transmitting,
            dir_cmds,
            dir_events,
            stations: Vec::new(),
            dir_status: "Logging in…".into(),
            dir_ok: false,
            state: State::Idle,
            remote_info: String::new(),
            log: VecDeque::new(),
            receiving: false,
            rx_since: None,
            tx_since: None,
            quitting: false,
            dtmf,
            rules,
            contacts: contacts::load(),
            active: None,
            pending_incoming: None,
            incoming: None,
            qrz_tx,
            qrz_rx,
            hidden: logbook::load_hidden(),
            pending_deletes: HashMap::new(),
            del_tx,
            del_rx,
            qrz_book: logbook::load(),
            book: Vec::new(),
            book_gen: 0,
            worked: HashMap::new(),
            sync: SyncState::Idle,
            last_sync: logbook::last_sync(),
            sync_tx,
            sync_rx,
            sync_seen: None,
            sync_new: 0,
            uploads_in_flight: 0,
            dir_done: false,
            exit_deadline: None,
        };
        let hidden = s.hidden.clone();
        s.qrz_book.retain(|q| !hidden.contains(&q.logid));
        s.rebuild_book();
        s.push(LogKind::System, format!("Audio: {}", s.audio.description));
        for note in s.audio.notes.clone() {
            s.push(LogKind::System, note);
        }
        Ok(s)
    }

    pub fn push(&mut self, kind: LogKind, text: String) {
        if kind == LogKind::System {
            crate::log!("ui: {text}");
        }
        for line in text.lines() {
            self.log.push_back((kind, line.to_string()));
        }
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }

    /// Handle background events.
    pub fn poll(&mut self) -> Update {
        let mut list_changed = false;
        while let Ok(e) = self.dir_events.try_recv() {
            match e {
                DirEvent::LoggedIn(s, route) => {
                    self.dir_ok = true;
                    let s = match s {
                        Status::Online => "Online",
                        Status::Busy => "Busy",
                        Status::Off => "Off",
                    };
                    self.dir_status = if route == "direct" { s.into() } else { format!("{s} · {route}") };
                }
                DirEvent::Stations(mut list) => {
                    list.sort_by(|a, b| a.callsign.cmp(&b.callsign));
                    let n = list.len();
                    self.stations = list;
                    list_changed = true;
                    self.push(LogKind::System, format!("Directory: {n} stations."));
                }
                DirEvent::Error(e) => {
                    self.dir_ok = false;
                    self.dir_status = "Error".into();
                    self.push(LogKind::System, format!("Directory: {e}"));
                }
                DirEvent::Done => self.dir_done = true,
            }
        }
        let mut changed = false;
        while let Ok(done) = self.qrz_rx.try_recv() {
            self.uploads_in_flight = self.uploads_in_flight.saturating_sub(1);
            if let Some(old) = done.replaced {
                self.qrz_book.retain(|q| q.logid != old);
                let _ = logbook::save(&self.qrz_book);
            }
            if let Some(note) = done.note {
                self.push(LogKind::System, note);
            }
            let Some(c) = self.contacts.iter_mut().find(|c| c.id() == done.id) else { continue };
            let call = c.call.clone();
            let line = match &done.status {
                Qrz::Sent(_) => format!("QRZ logbook: {call} logged."),
                Qrz::Failed(why) => format!("QRZ logbook: {call} not logged ({why})."),
                _ => String::new(),
            };
            if matches!(done.status, Qrz::Sent(_)) {
                c.replaces = None;
            }
            c.qrz = done.status;
            changed = true;
            if !line.is_empty() {
                self.push(LogKind::System, line);
            }
        }
        while let Ok((logid, result)) = self.del_rx.try_recv() {
            let Some(key) = self.pending_deletes.remove(&logid) else { continue };
            match result {
                Ok(()) => {
                    self.remove_entry(&key, false);
                    self.push(LogKind::System, "Deleted from your QRZ logbook and from this app.".into());
                }
                Err(e) => self.push(LogKind::System, format!("QRZ didn't delete it ({e}). It's still in your logbook.")),
            }
        }
        if changed {
            let _ = contacts::rewrite(&self.contacts);
            self.rebuild_book();
        }
        self.poll_sync();
        while let Ok(e) = self.events.try_recv() {
            match e {
                Event::State(s) => {
                    let connected = matches!(s, State::Connected { .. });
                    if !connected {
                        self.remote_info.clear();
                    }
                    self.track_contact(&s);
                    let _ = self.dir_cmds.send(DirCmd::Status(if connected { Status::Busy } else { Status::Online }));
                    self.state = s;
                }
                Event::Info(t) => self.remote_info = t,
                Event::Chat(t) => self.push(LogKind::Chat, t),
                Event::Receiving(r) => {
                    self.receiving = r;
                    self.rx_since = if r { self.rx_since.or(Some(Instant::now())) } else { None };
                }
                Event::Transmitting(t) => self.tx_since = t.then(Instant::now),
                Event::Log(t) => self.push(LogKind::System, t),
                Event::Incoming(call) => {
                    self.pending_incoming = Some(call.clone());
                    self.incoming = Some(call);
                }
            }
        }
        // Closing: finished once logged off, and uploads to QRZ have answered (or we've waited long enough).
        let waited_enough = self.exit_deadline.is_some_and(|d| Instant::now() > d);
        if self.dir_done && (self.uploads_in_flight == 0 || waited_enough) {
            return Update::Finished;
        }
        if list_changed { Update::StationsChanged } else { Update::Nothing }
    }

    /// Indices into `stations` matching a filter and search text
    /// (callsign, location, or exact node number).
    pub fn matching(&self, filter: Filter, query: &str) -> Vec<usize> {
        let q = query.trim().to_lowercase();
        (0..self.stations.len())
            .filter(|&i| {
                let s = &self.stations[i];
                filter.matches(s.kind())
                    && (q.is_empty()
                        || s.callsign.to_lowercase().contains(&q)
                        || s.description.to_lowercase().contains(&q)
                        || s.id.to_string() == q)
            })
            .collect()
    }

    pub fn find(&self, callsign: &str) -> Option<&Station> {
        self.stations.iter().find(|s| s.callsign.eq_ignore_ascii_case(callsign))
    }

    pub fn connected(&self) -> bool {
        matches!(self.state, State::Connected { .. })
    }

    pub fn transmitting(&self) -> bool {
        self.transmitting.load(Ordering::Relaxed)
    }

    pub fn connect(&mut self, s: &Station) {
        if s.ip.is_unspecified() {
            self.push(LogKind::System, format!("{} has no address in the directory.", s.callsign));
            return;
        }
        let label = format!("{} ({})", s.callsign, s.id);
        let _ = self.cmds.send(Cmd::Connect { ip: s.ip, label });
    }

    pub fn connect_node(&mut self, id: u32) {
        match self.stations.iter().find(|s| s.id == id).cloned() {
            Some(s) => self.connect(&s),
            None => self.push(LogKind::System, format!("Node {id} isn't online in the directory.")),
        }
    }

    pub fn disconnect(&mut self) {
        let _ = self.cmds.send(Cmd::Disconnect);
    }

    pub fn ptt(&mut self, on: bool) {
        if on && !self.connected() {
            self.push(LogKind::System, "Connect to a station before talking.".into());
            return;
        }
        let _ = self.cmds.send(Cmd::Ptt(on));
    }

    /// Send touch tones (0-9, A-D, *, #) to the station you're connected to.
    pub fn send_dtmf(&mut self, digits: &str) {
        let digits: String = digits.chars().filter(|&c| crate::dtmf::valid(c)).map(|c| c.to_ascii_uppercase()).collect();
        if digits.is_empty() {
            return;
        }
        if !self.connected() {
            self.push(LogKind::System, "Connect to a station before sending DTMF.".into());
            return;
        }
        self.push(LogKind::System, format!("DTMF sent: {digits}"));
        let _ = self.dtmf.send(digits);
    }

    /// Notice connections starting and ending, and keep the contact log.
    fn track_contact(&mut self, now: &State) {
        match now {
            State::Connected { callsign, name, .. } => {
                if self.active.as_ref().is_some_and(|a| &a.call == callsign) {
                    return;
                }
                self.end_contact();
                let incoming = self.pending_incoming.take().is_some_and(|c| &c == callsign);
                self.say_if_worked(callsign);
                self.active = Some(Active {
                    call: callsign.clone(),
                    name: name.clone(),
                    start: contacts::now(),
                    since: Instant::now(),
                    incoming,
                });
            }
            _ => self.end_contact(),
        }
    }

    fn end_contact(&mut self) {
        let Some(a) = self.active.take() else { return };
        let node = self.find(&a.call).map(|s| s.id);
        let mut c = Contact { call: a.call, name: a.name, node, start: a.start, secs: a.since.elapsed().as_secs(), incoming: a.incoming, qrz: Qrz::Pending, band: None, mode: None, replaces: None, echolink: true };
        if !c.is_qso() {
            c.qrz = Qrz::Skipped;
        }
        let _ = contacts::append(&c);
        if c.is_qso() {
            self.push(LogKind::System, format!("Logged {} ({}).", c.call, contacts::duration_label(c.secs)));
        }
        let (id, qso) = (c.id(), c.is_qso());
        self.contacts.push(c);
        self.rebuild_book();
        if qso && self.cfg.qrz_auto && self.qrz_key().is_some() {
            self.upload(&[id]);
        }
    }

    /// Rebuild the logbook view and the worked-before index.
    fn rebuild_book(&mut self) {
        self.book = logbook::entries(&self.qrz_book, &self.contacts);
        self.worked = logbook::worked_index(&self.qrz_book, &self.contacts);
        self.book_gen += 1;
    }

    /// Tell the user, in the station log, whether they've worked this station before.
    fn say_if_worked(&mut self, call: &str) {
        let kind_ok = !call.starts_with('*') && !call.to_uppercase().contains("ECHOTEST");
        if !kind_ok || (self.qrz_book.is_empty() && self.contacts.is_empty()) {
            return;
        }
        let line = match self.worked.get(&logbook::norm(call)) {
            Some(w) => format!(
                "You've worked {} {} time{} before; last {}.",
                logbook::norm(call),
                w.count,
                if w.count == 1 { "" } else { "s" },
                contacts::utc_label(w.last).split(' ').next().unwrap_or("")
            ),
            None => format!("First time you've worked {}.", logbook::norm(call)),
        };
        self.push(LogKind::System, line);
    }

    /// Copy your QRZ logbook to this Mac. Normally only contacts newer than the
    /// ones already here are fetched; `full` downloads everything and drops
    /// copies of contacts you've since deleted on QRZ.
    pub fn sync_qrz(&mut self, full: bool) {
        let Some(key) = self.qrz_key() else {
            self.push(LogKind::System, "Add your QRZ logbook key in Settings first.".into());
            return;
        };
        if matches!(self.sync, SyncState::Running(_)) {
            return;
        }
        let after = if full { 0 } else { self.qrz_book.iter().map(|q| q.logid).max().map(|m| m + 1).unwrap_or(0) };
        self.sync = SyncState::Running(0);
        self.sync_new = 0;
        self.sync_seen = full.then(HashSet::new);
        let tx = self.sync_tx.clone();
        std::thread::spawn(move || {
            let page_tx = tx.clone();
            let result = crate::qrz::Client::default().sync(&key, after, 250, |page| {
                let _ = page_tx.send(SyncMsg::Page(page));
            });
            let _ = tx.send(match result {
                Ok(_) => SyncMsg::Done,
                Err(e) => SyncMsg::Failed(e),
            });
        });
    }

    fn poll_sync(&mut self) {
        let mut finished = None;
        while let Ok(m) = self.sync_rx.try_recv() {
            match m {
                SyncMsg::Page(page) => {
                    let mut by_id: HashMap<u64, usize> = self.qrz_book.iter().enumerate().map(|(i, q)| (q.logid, i)).collect();
                    for q in page {
                        if self.hidden.contains(&q.logid) {
                            continue;
                        }
                        if let Some(seen) = &mut self.sync_seen {
                            seen.insert(q.logid);
                        }
                        match by_id.get(&q.logid) {
                            Some(&i) => self.qrz_book[i] = q,
                            None => {
                                by_id.insert(q.logid, self.qrz_book.len());
                                self.qrz_book.push(q);
                                self.sync_new += 1;
                            }
                        }
                    }
                    self.sync = SyncState::Running(self.sync_new);
                }
                SyncMsg::Done => finished = Some(None),
                SyncMsg::Failed(e) => finished = Some(Some(e)),
            }
        }
        let Some(failure) = finished else { return };
        match failure {
            None => {
                if let Some(seen) = self.sync_seen.take() {
                    self.qrz_book.retain(|q| seen.contains(&q.logid));
                }
                logbook::set_last_sync(contacts::now());
                self.last_sync = Some(contacts::now());
                self.sync = SyncState::Done { new: self.sync_new, total: self.qrz_book.len() };
                self.push(LogKind::System, format!("QRZ logbook synced: {} contacts ({} new).", self.qrz_book.len(), self.sync_new));
            }
            Some(e) => {
                self.sync_seen = None;
                self.sync = SyncState::Failed(e.clone());
                self.push(LogKind::System, format!("QRZ sync stopped: {e}"));
            }
        }
        self.qrz_book.sort_by(|a, b| a.logid.cmp(&b.logid));
        let _ = logbook::save(&self.qrz_book);
        self.rebuild_book();
    }

    /// Change a contact made in this app. If QRZ already had it, the edited version
    /// goes up and then the old copy is removed from QRZ.
    pub fn edit_contact(&mut self, local_id: &str, e: ContactEdit) -> Result<(), String> {
        let (band, mode) = (e.band.trim().to_string(), e.mode.trim().to_uppercase());
        let default = (self.cfg.log_band.clone(), self.cfg.log_mode.clone());
        let Some(c) = self.contacts.iter_mut().find(|c| c.id() == local_id) else { return Err("That contact is no longer in the log.".into()) };
        let old_logid = match &c.qrz {
            Qrz::Sent(id) => id.parse::<u64>().ok(),
            _ => None,
        };
        c.call = e.call.trim().to_uppercase();
        c.name = e.name.trim().to_string();
        c.start = e.start;
        c.secs = e.secs;
        c.band = (!band.is_empty() && band != default.0).then_some(band);
        c.mode = (!mode.is_empty() && mode != default.1).then_some(mode);
        c.echolink = e.echolink;
        if c.is_qso() {
            c.qrz = Qrz::Pending;
            c.replaces = old_logid.map(|id| id.to_string()).or(c.replaces.take());
        }
        let id = c.id();
        let qso = c.is_qso();
        let _ = contacts::rewrite(&self.contacts);
        self.rebuild_book();
        if qso && self.cfg.qrz_auto && self.qrz_key().is_some() {
            self.upload(&[id]);
        }
        Ok(())
    }

    /// Add a contact by hand.
    pub fn add_contact(&mut self, e: ContactEdit) -> Result<(), String> {
        let (band, mode) = (e.band.trim().to_string(), e.mode.trim().to_uppercase());
        let c = Contact {
            call: e.call.trim().to_uppercase(),
            name: e.name.trim().to_string(),
            node: None,
            start: e.start,
            secs: e.secs,
            incoming: false,
            qrz: Qrz::Pending,
            band: (!band.is_empty() && band != self.cfg.log_band).then_some(band),
            mode: (!mode.is_empty() && mode != self.cfg.log_mode).then_some(mode),
            replaces: None,
            echolink: e.echolink,
        };
        if self.contacts.iter().any(|x| x.id() == c.id()) {
            return Err("You already have a contact with that station at that time.".into());
        }
        let id = c.id();
        let _ = contacts::append(&c);
        self.contacts.push(c);
        self.rebuild_book();
        self.push(LogKind::System, format!("Added {} to your logbook.", self.contacts.last().map(|c| c.call.clone()).unwrap_or_default()));
        if self.cfg.qrz_auto && self.qrz_key().is_some() {
            self.upload(&[id]);
        }
        Ok(())
    }

    /// Remove a logbook entry. `also_qrz` deletes it from your QRZ logbook first
    /// (permanent; the entry goes from here only if QRZ agrees). Without it, an entry QRZ
    /// has is just hidden in this app, and stays hidden through re-syncs.
    pub fn delete_entry(&mut self, key: &str, also_qrz: bool) {
        let Some(entry) = self.book.iter().find(|e| e.key == key).cloned() else { return };
        if also_qrz && let Some(logid) = entry.logid {
            let Some(qrz_key) = self.qrz_key() else {
                self.push(LogKind::System, "Add your QRZ logbook key in Settings to delete from QRZ.".into());
                return;
            };
            self.pending_deletes.insert(logid, key.to_string());
            let tx = self.del_tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send((logid, crate::qrz::Client::default().delete(&qrz_key, &[logid])));
            });
            self.push(LogKind::System, format!("Deleting {} from QRZ…", entry.call));
            return;
        }
        self.remove_entry(key, true);
        self.push(LogKind::System, format!("Removed {} from your logbook here.", entry.call));
    }

    /// Drop an entry from this app. `hide` remembers QRZ's copy so a re-sync leaves it out.
    fn remove_entry(&mut self, key: &str, hide: bool) {
        let logid = self.book.iter().find(|e| e.key == key).and_then(|e| e.logid);
        if let Some(local_id) = key.strip_prefix("L:") {
            self.contacts.retain(|c| c.id() != local_id);
            let _ = contacts::rewrite(&self.contacts);
        }
        if let Some(id) = logid {
            self.qrz_book.retain(|q| q.logid != id);
            let _ = logbook::save(&self.qrz_book);
            if hide {
                self.hidden.insert(id);
                logbook::save_hidden(&self.hidden);
            }
        }
        self.rebuild_book();
    }

    /// Stop hiding entries; a full re-sync then brings them back.
    pub fn restore_hidden(&mut self) {
        self.hidden.clear();
        logbook::save_hidden(&self.hidden);
        self.push(LogKind::System, "Hidden entries restored. Press FULL RE-SYNC to bring them back from QRZ.".into());
    }

    /// The QRZ access key, if one is set.
    pub fn qrz_key(&self) -> Option<String> {
        self.cfg.qrz_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(String::from)
    }

    /// Send these contacts to QRZ in the background, one at a time.
    fn upload(&mut self, ids: &[String]) {
        let Some(key) = self.qrz_key() else { return };
        let jobs: Vec<(String, String, Option<u64>)> = ids
            .iter()
            .filter_map(|id| self.contacts.iter().find(|c| &c.id() == id))
            .map(|c| (c.id(), c.adif(&self.cfg.callsign, &self.cfg.log_band, &self.cfg.log_mode), c.replaces.as_deref().and_then(|r| r.parse().ok())))
            .collect();
        let tx = self.qrz_tx.clone();
        self.uploads_in_flight += jobs.len();
        std::thread::spawn(move || {
            let client = crate::qrz::Client::default();
            for (n, (id, adif, replaces)) in jobs.into_iter().enumerate() {
                if n > 0 {
                    std::thread::sleep(Duration::from_millis(400)); // be polite to QRZ
                }
                let (mut replaced, mut note) = (None, None);
                let status = match client.insert(&key, &adif) {
                    Ok(logid) => {
                        // An edited contact: the new copy is up, so the old one can go.
                        if let Some(old) = replaces {
                            match client.delete(&key, &[old]) {
                                Ok(()) => replaced = Some(old),
                                Err(e) => note = Some(format!("The edited contact is on QRZ, but the old copy couldn't be removed ({e}). Delete it on QRZ.")),
                            }
                        }
                        Qrz::Sent(logid)
                    }
                    Err(why) => Qrz::Failed(why),
                };
                if tx.send(UploadDone { id, status, replaced, note }).is_err() {
                    return;
                }
            }
        });
    }

    /// Upload every logged contact that QRZ doesn't have yet. Returns how many.
    pub fn upload_pending(&mut self) -> usize {
        if self.qrz_key().is_none() {
            self.push(LogKind::System, "Add your QRZ logbook key in Settings first.".into());
            return 0;
        }
        let n = self.upload_unsent();
        if n > 0 {
            self.push(LogKind::System, format!("Sending {n} contact(s) to QRZ…"));
        }
        n
    }

    /// Start uploading every contact QRZ doesn't have, without saying anything.
    fn upload_unsent(&mut self) -> usize {
        let ids: Vec<String> = self
            .contacts
            .iter()
            .filter(|c| c.is_qso() && matches!(c.qrz, Qrz::Pending | Qrz::Failed(_)))
            .map(|c| c.id())
            .collect();
        if !ids.is_empty() {
            self.upload(&ids);
        }
        ids.len()
    }

    /// Write the contact log as an ADIF file in Documents and return where it went.
    pub fn export_adif(&self) -> std::io::Result<PathBuf> {
        let dir = dirs::document_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."));
        let (date, _) = contacts::utc(contacts::now());
        let path = dir.join(format!("EchoLink-log-{date}.adi"));
        std::fs::write(&path, contacts::adif_file(&self.contacts, &self.cfg.callsign, &self.cfg.log_band, &self.cfg.log_mode))?;
        Ok(path)
    }

    /// Stations you've talked to lately, newest first.
    pub fn recent(&self, n: usize) -> Vec<&Contact> {
        contacts::recent(&self.contacts, n)
    }

    /// Connect to a station by its directory callsign.
    pub fn connect_call(&mut self, call: &str) {
        match self.find(call).cloned() {
            Some(s) => self.connect(&s),
            None => self.push(LogKind::System, format!("{call} isn't online in the directory right now.")),
        }
    }

    /// Never accept calls from `call` again.
    pub fn block(&mut self, call: &str) {
        let call = call.trim().to_uppercase();
        let base = call.strip_suffix("-R").or_else(|| call.strip_suffix("-L")).unwrap_or(&call).to_string();
        if !self.cfg.blocked.contains(&base) {
            self.cfg.blocked.push(base.clone());
        }
        self.rules.write().unwrap().blocked = self.cfg.blocked.clone();
        let blocked = self.cfg.blocked.clone();
        let _ = crate::config::update(|c| c.blocked = blocked);
        self.push(LogKind::System, format!("{base} is blocked. Edit the list in Settings."));
    }

    /// Do Not Disturb on or off.
    pub fn set_dnd(&mut self, on: bool) {
        self.cfg.dnd = on;
        self.rules.write().unwrap().dnd = on;
        let _ = crate::config::update(|c| c.dnd = on);
        self.push(LogKind::System, if on { "Do Not Disturb on: incoming calls are refused.".into() } else { "Do Not Disturb off.".into() });
    }

    pub fn chat(&mut self, text: &str) {
        let text = text.trim();
        if !text.is_empty() {
            let _ = self.cmds.send(Cmd::Chat(text.into()));
        }
    }

    pub fn refresh(&mut self) {
        self.push(LogKind::System, "Refreshing the station list…".into());
        let _ = self.dir_cmds.send(DirCmd::Refresh);
    }

    /// Disconnect and log off; `poll()` returns `Err` when that's done.
    pub fn quit(&mut self) {
        if !self.quitting {
            self.end_contact();
            if self.cfg.qrz_on_exit && self.qrz_key().is_some() {
                let n = self.upload_unsent();
                if n > 0 {
                    self.push(LogKind::System, format!("Sending {n} unsent contact(s) to QRZ before closing…"));
                }
            }
            // Don't close on an upload that is still talking to QRZ.
            if self.uploads_in_flight > 0 {
                self.exit_deadline = Some(Instant::now() + Duration::from_secs(20));
            }
            self.quitting = true;
            let _ = self.cmds.send(Cmd::Quit);
            let _ = self.dir_cmds.send(DirCmd::Quit);
            self.dir_status = "Logging off…".into();
        }
    }
}
