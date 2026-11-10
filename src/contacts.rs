//! The contact log: every connection you make or take, kept on disk, exported
//! as ADIF, and (optionally) uploaded to your QRZ logbook.

use crate::directory::Kind;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

/// Connections shorter than this are kept in the history but not logged as QSOs.
pub const MIN_QSO_SECS: u64 = 10;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(tag = "state", content = "detail")]
pub enum Qrz {
    /// Not uploaded yet.
    #[default]
    Pending,
    /// In your QRZ logbook (the log id, or "duplicate").
    Sent(String),
    /// QRZ refused it or couldn't be reached; the reason.
    Failed(String),
    /// Not a QSO (conference, echo test, too short), so never uploaded.
    Skipped,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Contact {
    /// As the directory shows it, e.g. `W6XYZ`, `W6XYZ-R`, `*CONFERENCE*`.
    pub call: String,
    /// Operator name or station description from the far end.
    pub name: String,
    pub node: Option<u32>,
    /// Start, Unix seconds (UTC).
    pub start: i64,
    pub secs: u64,
    /// They called us.
    pub incoming: bool,
    #[serde(default)]
    pub qrz: Qrz,
    /// Band and mode if they differ from your default ones in Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub band: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// After an edit: the QRZ log id of the old copy, to remove once the new one is up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
    /// Made over EchoLink. Contacts added by hand can be anything (an RF contact, say),
    /// and then aren't tagged as EchoLink in the ADIF.
    #[serde(default = "yes")]
    pub echolink: bool,
}

fn yes() -> bool {
    true
}

impl Contact {
    pub fn kind(&self) -> Kind {
        let c = &self.call;
        if c.starts_with('*') {
            Kind::Conference
        } else if c.ends_with("-R") {
            Kind::Repeater
        } else if c.ends_with("-L") {
            Kind::Link
        } else {
            Kind::User
        }
    }

    /// The call sign without the `-R` / `-L` node suffix.
    pub fn base_call(&self) -> &str {
        self.call.strip_suffix("-R").or_else(|| self.call.strip_suffix("-L")).unwrap_or(&self.call)
    }

    /// A real contact worth a log entry: not a conference, not the echo test,
    /// and long enough to mean something.
    pub fn is_qso(&self) -> bool {
        self.kind() != Kind::Conference && !self.call.to_uppercase().contains("ECHOTEST") && self.secs >= MIN_QSO_SECS
    }

    /// Unique enough to find this contact again.
    pub fn id(&self) -> String {
        format!("{}/{}", self.start, self.call)
    }

    /// One ADIF record.
    pub fn adif(&self, my_call: &str, band: &str, mode: &str) -> String {
        let (d1, t1) = utc(self.start);
        let (d2, t2) = utc(self.start + self.secs as i64);
        let mut out = String::new();
        let mut f = |name: &str, v: &str| {
            let v = ascii(v);
            if !v.is_empty() {
                out.push_str(&format!("<{name}:{}>{v} ", v.len()));
            }
        };
        f("CALL", self.base_call());
        f("QSO_DATE", &d1);
        f("TIME_ON", &t1);
        f("QSO_DATE_OFF", &d2);
        f("TIME_OFF", &t2);
        f("BAND", self.band.as_deref().unwrap_or(band));
        f("MODE", self.mode.as_deref().unwrap_or(mode));
        if self.echolink {
            f("PROP_MODE", "ECH"); // ADIF's code for EchoLink
        }
        f("STATION_CALLSIGN", my_call);
        if self.kind() == Kind::User || !self.echolink {
            f("NAME", &self.name);
        }
        let mut comment = String::new();
        if self.echolink {
            comment.push_str("EchoLink");
            if let Some(n) = self.node {
                comment.push_str(&format!(" node {n}"));
            }
            if self.kind() != Kind::User && !self.name.is_empty() {
                comment.push_str(&format!(", {}", self.name));
            }
            if self.incoming {
                comment.push_str(", they called me");
            }
        }
        f("COMMENT", &comment);
        out.push_str("<EOR>\n");
        out
    }
}

/// An ADIF file with every logged contact in `contacts`.
pub fn adif_file(contacts: &[Contact], my_call: &str, band: &str, mode: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let mut out = format!(
        "EchoLink for Mac contact log\n<ADIF_VER:5>3.1.4 <PROGRAMID:13>EchoLink-Mac <PROGRAMVERSION:{}>{version} <EOH>\n\n",
        version.len()
    );
    for c in contacts.iter().filter(|c| c.is_qso()) {
        out.push_str(&c.adif(my_call, band, mode));
    }
    out
}

/// ADIF counts bytes, so keep it to plain ASCII and no line breaks.
fn ascii(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii() && !c.is_ascii_control()).collect::<String>().trim().to_string()
}

/// `(YYYYMMDD, HHMMSS)` in UTC for a Unix time.
pub fn utc(t: i64) -> (String, String) {
    let (y, m, d, h, mi, s) = utc_parts(t);
    (format!("{y:04}{m:02}{d:02}"), format!("{h:02}{mi:02}{s:02}"))
}

/// `2026-11-09` for a Unix time.
pub fn date_str(t: i64) -> String {
    let (y, m, d, ..) = utc_parts(t);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `18:04:09` for a Unix time.
pub fn time_str(t: i64) -> String {
    let (.., h, mi, s) = utc_parts(t);
    format!("{h:02}:{mi:02}:{s:02}")
}

/// Unix time from what a person typed: `2026-11-09` and `18:04` or `18:04:09` (UTC).
pub fn parse_datetime(date: &str, time: &str) -> Option<i64> {
    let mut d = date.trim().split('-');
    let (y, m, day): (i64, i64, i64) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    if d.next().is_some() || !(1970..=2200).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    let mut t = time.trim().split(':');
    let (h, mi): (i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let s: i64 = t.next().map(|s| s.parse().ok()).unwrap_or(Some(0))?;
    if t.next().is_some() || !(0..24).contains(&h) || !(0..60).contains(&mi) || !(0..60).contains(&s) {
        return None;
    }
    let unix = from_utc(y, m, day, h, mi, s);
    // Reject dates the calendar doesn't have, like 2026-02-31.
    (date_str(unix) == format!("{y:04}-{m:02}-{day:02}")).then_some(unix)
}

/// Seconds from `90`, `1:30` or `1:02:03`.
pub fn parse_duration(text: &str) -> Option<u64> {
    let parts: Vec<u64> = text.trim().split(':').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    match parts.as_slice() {
        [s] => Some(*s),
        [m, s] if *s < 60 => Some(m * 60 + s),
        [h, m, s] if *m < 60 && *s < 60 => Some(h * 3600 + m * 60 + s),
        _ => None,
    }
}

/// `2026-11-09 18:04` style text for the log window.
pub fn utc_label(t: i64) -> String {
    let (y, m, d, h, mi, _) = utc_parts(t);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
}

fn utc_parts(t: i64) -> (i64, i64, i64, i64, i64, i64) {
    let days = t.div_euclid(86400);
    let rem = t.rem_euclid(86400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Unix seconds for a UTC date and time (the reverse of [`utc`]).
pub fn from_utc(y: i64, m: i64, d: i64, h: i64, mi: i64, s: i64) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86400 + h * 3600 + mi * 60 + s
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `1:05:09` or `4:30`.
pub fn duration_label(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

pub fn path() -> PathBuf {
    crate::config::path().with_file_name("contacts.jsonl")
}

/// The saved history, oldest first. Lines that don't parse are skipped.
pub fn load() -> Vec<Contact> {
    let Ok(text) = std::fs::read_to_string(path()) else { return Vec::new() };
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

pub fn append(c: &Contact) -> std::io::Result<()> {
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = open_private(&p, true)?;
    writeln!(f, "{}", serde_json::to_string(c)?)
}

/// Replace the whole file (after an upload status changes).
pub fn rewrite(all: &[Contact]) -> std::io::Result<()> {
    let p = path();
    let tmp = p.with_extension("jsonl.tmp");
    {
        let mut f = open_private(&tmp, false)?;
        for c in all {
            writeln!(f, "{}", serde_json::to_string(c)?)?;
        }
    }
    std::fs::rename(tmp, p)
}

fn open_private(p: &std::path::Path, append: bool) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).mode(0o600);
    if append {
        o.append(true);
    } else {
        o.truncate(true);
    }
    o.open(p)
}

/// The last `n` different stations you've been connected to, newest first.
pub fn recent(all: &[Contact], n: usize) -> Vec<&Contact> {
    let mut seen = std::collections::HashSet::new();
    all.iter().rev().filter(|c| seen.insert(c.call.clone())).take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(call: &str, secs: u64) -> Contact {
        Contact { call: call.into(), name: "Pat".into(), node: Some(12345), start: 1_700_000_000, secs, incoming: false, qrz: Qrz::Pending, band: None, mode: None, replaces: None, echolink: true }
    }

    #[test]
    fn utc_conversion() {
        assert_eq!(utc(0), ("19700101".into(), "000000".into()));
        assert_eq!(utc(1_700_000_000), ("20231114".into(), "221320".into())); // known value
        assert_eq!(utc(951_782_400).0, "20000229"); // leap day
        assert_eq!(utc_label(1_700_000_000), "2023-11-14 22:13");
    }

    #[test]
    fn adif_record_matches_the_spec_shape() {
        let a = contact("K6ABC", 125).adif("KI6UIT", "2m", "FM");
        for want in [
            "<CALL:5>K6ABC",
            "<QSO_DATE:8>20231114",
            "<TIME_ON:6>221320",
            "<TIME_OFF:6>221525",
            "<BAND:2>2m",
            "<MODE:2>FM",
            "<PROP_MODE:3>ECH",
            "<STATION_CALLSIGN:6>KI6UIT",
            "<NAME:3>Pat",
            "node 12345",
        ] {
            assert!(a.contains(want), "{want} missing from {a}");
        }
        assert!(a.trim_end().ends_with("<EOR>"));
        // Repeaters log under their base call, with the description as a comment, not a name.
        let r = contact("W6XYZ-R", 60).adif("KI6UIT", "2m", "FM");
        assert!(r.contains("<CALL:5>W6XYZ") && !r.contains("<NAME:"), "{r}");
        assert!(r.contains("Pat"), "{r}");
    }

    #[test]
    fn a_contacts_own_band_and_mode_win() {
        let mut c = contact("K6ABC", 125);
        c.band = Some("70cm".into());
        c.mode = Some("DMR".into());
        let a = c.adif("KI6UIT", "2m", "FM");
        assert!(a.contains("<BAND:4>70cm") && a.contains("<MODE:3>DMR") && !a.contains("<BAND:2>2m"), "{a}");
    }

    #[test]
    fn contacts_added_by_hand_need_not_be_echolink() {
        let mut c = contact("K6ABC", 125);
        c.echolink = false;
        c.node = None;
        let a = c.adif("KI6UIT", "2m", "FM");
        assert!(!a.contains("PROP_MODE") && !a.contains("EchoLink") && a.contains("<NAME:3>Pat") && a.contains("<CALL:5>K6ABC"), "{a}");
        // Old log lines without the flag are EchoLink contacts.
        let old = r#"{"call":"K6ABC","name":"","node":null,"start":5,"secs":9,"incoming":false}"#;
        assert!(serde_json::from_str::<Contact>(old).unwrap().echolink);
    }

    #[test]
    fn typed_dates_times_and_durations() {
        assert_eq!(parse_datetime("2023-11-14", "22:13:20"), Some(1_700_000_000));
        assert_eq!(parse_datetime(" 2023-11-14 ", "22:13"), Some(1_699_999_980));
        for bad in [("2023-02-31", "10:00"), ("2023-13-01", "10:00"), ("2023-11-14", "25:00"), ("2023-11-14", "10:61"), ("yesterday", "10:00"), ("2023-11-14", "noon"), ("2023-11", "10:00")] {
            assert_eq!(parse_datetime(bad.0, bad.1), None, "{bad:?}");
        }
        assert_eq!((date_str(1_700_000_000), time_str(1_700_000_000)), ("2023-11-14".into(), "22:13:20".into()));
        assert_eq!((parse_duration("90"), parse_duration("1:30"), parse_duration("1:02:03")), (Some(90), Some(90), Some(3723)));
        assert_eq!((parse_duration("1:75"), parse_duration("x"), parse_duration("")), (None, None, None));
    }

    #[test]
    fn adif_lengths_count_bytes_and_drop_non_ascii() {
        let mut c = contact("K6ABC", 60);
        c.name = "José\n".into();
        let a = c.adif("KI6UIT", "2m", "FM");
        assert!(a.contains("<NAME:3>Jos"), "{a}");
    }

    #[test]
    fn only_real_qsos_are_exported() {
        let all = [contact("K6ABC", 125), contact("*ECHOTEST*", 300), contact("*CONF*", 600), contact("K6DEF", 3), contact("W1AW-L", 40)];
        let file = adif_file(&all, "KI6UIT", "2m", "FM");
        assert_eq!(file.matches("<EOR>").count(), 2, "{file}");
        assert!(file.contains("<EOH>") && file.contains("<ADIF_VER:5>3.1.4"));
    }

    #[test]
    fn recent_is_newest_first_and_unique() {
        let mut a = contact("K6AAA", 30);
        a.start = 1;
        let mut b = contact("K6BBB", 30);
        b.start = 2;
        let mut c = contact("K6AAA", 30);
        c.start = 3;
        let all = [a, b, c];
        let r: Vec<_> = recent(&all, 5).iter().map(|c| c.call.as_str()).collect();
        assert_eq!(r, ["K6AAA", "K6BBB"]);
    }

    #[test]
    fn json_round_trip_keeps_status() {
        let mut c = contact("K6ABC", 125);
        c.qrz = Qrz::Failed("bad band".into());
        let line = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Contact>(&line).unwrap(), c);
        // Older lines without a status still load.
        let old = r#"{"call":"K6ABC","name":"","node":null,"start":5,"secs":9,"incoming":false}"#;
        assert_eq!(serde_json::from_str::<Contact>(old).unwrap().qrz, Qrz::Pending);
    }
}
