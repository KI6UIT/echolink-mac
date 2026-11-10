use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    /// Your validated EchoLink callsign, without -L/-R for a personal station.
    pub callsign: String,
    pub password: String,
    pub name: String,
    /// Shown in the directory, at most 27 characters.
    pub location: String,
    #[serde(default = "default_servers")]
    pub servers: Vec<String>,
    /// Let other stations connect to you while you're idle.
    #[serde(default = "yes")]
    pub accept_incoming: bool,
    /// EchoLink proxy for networks where UDP 5198-5199 can't reach this
    /// computer (Starlink, cellular, carrier-grade NAT): "auto" picks a public
    /// proxy, or give "host" / "host:port". Leave out to connect directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// Password for a private proxy; public proxies use "PUBLIC".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_password: Option<String>,
    /// Starred stations (callsigns), shown first in the desktop app.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub favorites: Vec<String>,
    /// Software gain on the microphone, in dB (0 = as the sound card delivers it).
    #[serde(default)]
    pub mic_gain_db: f32,
    /// Automatic gain control: keeps your voice in the yellow and out of the red.
    #[serde(default)]
    pub auto_gain: bool,
    /// Remove background (room) noise from your microphone.
    #[serde(default)]
    pub noise_reduction: bool,
    /// Microphone to use, by name; leave out for the system default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_device: Option<String>,
    /// Speaker to use, by name; leave out for the system default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_device: Option<String>,
    /// Speaker volume, 1.0 = as received.
    #[serde(default = "full_volume")]
    pub speaker_volume: f32,
    /// Hold this key combination to talk, from any app (macOS), e.g. "ctrl+alt+space".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ptt_hotkey: Option<String>,
    /// Your QRZ logbook access key (from the QRZ website, under your logbook's settings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qrz_key: Option<String>,
    /// Send each contact to QRZ as soon as it ends.
    #[serde(default = "yes")]
    pub qrz_auto: bool,
    /// When you quit, send any contacts QRZ doesn't have yet (waits for it to finish).
    #[serde(default)]
    pub qrz_on_exit: bool,
    /// Your QRZ.com login, for looking up callsigns (separate from the logbook key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qrz_user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qrz_pass: Option<String>,
    /// QRZ and ADIF need a band and mode; EchoLink has neither, so these are what gets logged.
    #[serde(default = "default_band")]
    pub log_band: String,
    #[serde(default = "default_mode")]
    pub log_mode: String,
    /// Calls and prefixes that can never connect to you: `K6ABC`, `W1*`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<String>,
    /// With `allow_only`, only these can connect.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed: Vec<String>,
    #[serde(default)]
    pub allow_only: bool,
    /// Do Not Disturb: refuse every incoming call.
    #[serde(default)]
    pub dnd: bool,
}

/// Who may connect to us.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    pub blocked: Vec<String>,
    pub allowed: Vec<String>,
    pub allow_only: bool,
    pub dnd: bool,
}

impl Rules {
    pub fn from_config(c: &Config) -> Rules {
        Rules { blocked: c.blocked.clone(), allowed: c.allowed.clone(), allow_only: c.allow_only, dnd: c.dnd }
    }

    /// `Ok` if `call` may connect, otherwise why not.
    pub fn permits(&self, call: &str) -> Result<(), &'static str> {
        if self.dnd {
            return Err("Do Not Disturb is on");
        }
        if self.blocked.iter().any(|p| call_matches(p, call)) {
            return Err("it's on your block list");
        }
        if self.allow_only && !self.allowed.iter().any(|p| call_matches(p, call)) {
            return Err("it's not on your allow list");
        }
        Ok(())
    }
}

/// `K6ABC` matches that call (with or without a `-R`/`-L` suffix); `W1*` matches a prefix.
pub fn call_matches(pattern: &str, call: &str) -> bool {
    let p = pattern.trim().to_uppercase();
    let c = call.trim().to_uppercase();
    let base = c.strip_suffix("-R").or_else(|| c.strip_suffix("-L")).unwrap_or(&c);
    match p.strip_suffix('*') {
        Some(prefix) => !prefix.is_empty() && base.starts_with(prefix),
        None => !p.is_empty() && (p == c || p == base),
    }
}

/// Split what a person typed into a list: spaces, commas or new lines.
pub fn parse_list(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in text.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        let w = w.trim().to_uppercase();
        if !w.is_empty() && !out.contains(&w) {
            out.push(w);
        }
    }
    out
}

/// Change the saved settings in place (so other fields on disk are kept).
pub fn update(f: impl FnOnce(&mut Config)) -> Result<()> {
    let mut c = load()?;
    f(&mut c);
    save(&c)
}

fn default_band() -> String {
    "2m".into()
}

fn default_mode() -> String {
    "FM".into()
}

impl Config {
    /// Settings for a new station, before the user fills them in.
    pub fn blank() -> Config {
        Config {
            callsign: String::new(),
            password: String::new(),
            name: String::new(),
            location: String::new(),
            servers: default_servers(),
            accept_incoming: true,
            proxy: None,
            proxy_password: None,
            favorites: Vec::new(),
            mic_gain_db: 0.0,
            auto_gain: false,
            noise_reduction: false,
            input_device: None,
            output_device: None,
            speaker_volume: 1.0,
            ptt_hotkey: None,
            qrz_key: None,
            qrz_auto: true,
            qrz_on_exit: false,
            qrz_user: None,
            qrz_pass: None,
            log_band: default_band(),
            log_mode: default_mode(),
            blocked: Vec::new(),
            allowed: Vec::new(),
            allow_only: false,
            dnd: false,
        }
    }
}

/// Write the settings, readable only by you.
pub fn save(cfg: &Config) -> Result<()> {
    let p = path();
    std::fs::create_dir_all(p.parent().unwrap())?;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&p)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(toml::to_string(cfg)?.as_bytes())?;
    Ok(())
}

fn default_servers() -> Vec<String> {
    ["servers.echolink.org", "naeast.echolink.org", "nasouth.echolink.org", "backup.echolink.org"]
        .map(String::from)
        .to_vec()
}

fn full_volume() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

pub fn path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".config")).join("echolink-tui/config.toml")
}

pub fn load() -> Result<Config> {
    let p = path();
    let text = std::fs::read_to_string(&p)
        .with_context(|| format!("no settings at {}; run `echolink-tui setup` first", p.display()))?;
    let mode = std::fs::metadata(&p)?.permissions().mode();
    if mode & 0o077 != 0 {
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
    }
    let mut c: Config = toml::from_str(&text).with_context(|| format!("reading {}", p.display()))?;
    if let Ok(pw) = std::env::var("ECHOLINK_PASSWORD") {
        c.password = pw;
    }
    c.callsign = c.callsign.trim().to_uppercase();
    if c.callsign.is_empty() || c.password.is_empty() {
        bail!("callsign and password must be set in {}", p.display());
    }
    Ok(c)
}

fn ask(prompt: &str, default: &str) -> Result<String> {
    if default.is_empty() {
        print!("{prompt}: ");
    } else {
        print!("{prompt} [{default}]: ");
    }
    std::io::stdout().flush()?;
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    let s = s.trim();
    Ok(if s.is_empty() { default.to_string() } else { s.to_string() })
}

/// Interactive first-time setup. The password is read without echo and
/// stored in a file only you can read.
pub fn setup() -> Result<()> {
    let old = load().ok();
    let get = |f: fn(&Config) -> &str| old.as_ref().map(f).unwrap_or("").to_string();
    println!("EchoLink station setup. Your callsign must be validated at echolink.org.");
    println!("Use your personal callsign (e.g. N0CALL), not your repeater's -R node.\n");
    let callsign = ask("Callsign", &get(|c| &c.callsign))?.to_uppercase();
    let name = ask("Your name", &get(|c| &c.name))?;
    let location = ask("Location (max 27 chars)", &get(|c| &c.location))?;
    println!("\nOn Starlink, cellular or other shared-IP internet, EchoLink needs a proxy.");
    let proxy = ask("Proxy: auto, host[:port], or none", old.as_ref().and_then(|c| c.proxy.as_deref()).unwrap_or("none"))?;
    let proxy = (!proxy.eq_ignore_ascii_case("none") && !proxy.is_empty()).then_some(proxy);
    let keep = old.is_some();
    let password = loop {
        let prompt = if keep { "EchoLink password (Enter keeps current): " } else { "EchoLink password: " };
        let pw = rpassword::prompt_password(prompt)?;
        if pw.is_empty() && keep {
            break old.as_ref().unwrap().password.clone();
        }
        if pw.is_empty() {
            continue;
        }
        if rpassword::prompt_password("Again: ")? == pw {
            break pw;
        }
        println!("They don't match, try again.");
    };
    let cfg = Config {
        callsign,
        password,
        name,
        location: location.chars().take(27).collect(),
        servers: old.as_ref().map(|c| c.servers.clone()).unwrap_or_else(default_servers),
        accept_incoming: old.as_ref().map(|c| c.accept_incoming).unwrap_or(true),
        proxy,
        proxy_password: old.as_ref().and_then(|c| c.proxy_password.clone()),
        favorites: old.as_ref().map(|c| c.favorites.clone()).unwrap_or_default(),
        mic_gain_db: old.as_ref().map(|c| c.mic_gain_db).unwrap_or(0.0),
        auto_gain: old.as_ref().map(|c| c.auto_gain).unwrap_or(false),
        noise_reduction: old.as_ref().map(|c| c.noise_reduction).unwrap_or(false),
        input_device: old.as_ref().and_then(|c| c.input_device.clone()),
        output_device: old.as_ref().and_then(|c| c.output_device.clone()),
        speaker_volume: old.as_ref().map(|c| c.speaker_volume).unwrap_or(1.0),
        ptt_hotkey: old.as_ref().and_then(|c| c.ptt_hotkey.clone()),
        qrz_key: old.as_ref().and_then(|c| c.qrz_key.clone()),
        qrz_auto: old.as_ref().map(|c| c.qrz_auto).unwrap_or(true),
        qrz_on_exit: old.as_ref().map(|c| c.qrz_on_exit).unwrap_or(false),
        qrz_user: old.as_ref().and_then(|c| c.qrz_user.clone()),
        qrz_pass: old.as_ref().and_then(|c| c.qrz_pass.clone()),
        log_band: old.as_ref().map(|c| c.log_band.clone()).unwrap_or_else(default_band),
        log_mode: old.as_ref().map(|c| c.log_mode.clone()).unwrap_or_else(default_mode),
        blocked: old.as_ref().map(|c| c.blocked.clone()).unwrap_or_default(),
        allowed: old.as_ref().map(|c| c.allowed.clone()).unwrap_or_default(),
        allow_only: old.as_ref().map(|c| c.allow_only).unwrap_or(false),
        dnd: old.as_ref().map(|c| c.dnd).unwrap_or(false),
    };
    save(&cfg)?;
    println!("\nSaved to {} (readable only by you).", path().display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_matching() {
        assert!(call_matches("K6ABC", "K6ABC"));
        assert!(call_matches("k6abc", "K6ABC-R"), "node suffix and case don't matter");
        assert!(call_matches("W1*", "W1AW"));
        assert!(call_matches("W1*", "W1AW-L"));
        assert!(!call_matches("W1*", "KW1AW"));
        assert!(!call_matches("K6AB", "K6ABC"), "a bare call is exact, not a prefix");
        assert!(!call_matches("*", "K6ABC") && !call_matches("", "K6ABC"));
    }

    #[test]
    fn rules() {
        let mut r = Rules { blocked: parse_list("N0BAD, W1*"), ..Default::default() };
        assert!(r.permits("K6ABC").is_ok());
        assert!(r.permits("N0BAD").is_err() && r.permits("W1AW").is_err());
        r.allow_only = true;
        r.allowed = parse_list("K6ABC\nK6DEF");
        assert!(r.permits("K6ABC").is_ok() && r.permits("K6XYZ").is_err());
        r.dnd = true;
        assert_eq!(r.permits("K6ABC"), Err("Do Not Disturb is on"));
    }

    #[test]
    fn list_parsing() {
        assert_eq!(parse_list(" k6abc,w1*;\nk6abc  n0call "), ["K6ABC", "W1*", "N0CALL"]);
        assert!(parse_list("  ").is_empty());
    }

    #[test]
    fn old_config_files_still_load() {
        let old = "callsign='K6ABC'\npassword='x'\nname='A'\nlocation='B'\n";
        let c: Config = toml::from_str(old).unwrap();
        assert!(c.qrz_key.is_none() && c.qrz_auto && !c.qrz_on_exit && c.log_band == "2m" && c.log_mode == "FM" && !c.dnd);
    }
}
