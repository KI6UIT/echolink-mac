//! Your QRZ logbook, copied to this Mac: reading ADIF, storing the copy, and
//! answering "have I worked them before?".

use crate::contacts::{self, Contact};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// One contact as it is in the QRZ logbook.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct QrzQso {
    /// QRZ's number for this record.
    pub logid: u64,
    pub call: String,
    #[serde(default)]
    pub name: String,
    /// Start, Unix seconds (UTC).
    pub when: i64,
    #[serde(default)]
    pub band: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub comment: String,
}

/// Records in an ADIF string, each as FIELD -> value. Field names are upper-cased.
/// Lengths in ADIF count bytes, so that is what is used here.
pub fn parse_adif(text: &str) -> Vec<HashMap<String, String>> {
    let b = text.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    let mut rec: HashMap<String, String> = HashMap::new();
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let Some(close) = b[i..].iter().position(|&c| c == b'>') else { break };
        let tag = String::from_utf8_lossy(&b[i + 1..i + close]).into_owned();
        i += close + 1;
        let mut parts = tag.split(':');
        let name = parts.next().unwrap_or("").trim().to_uppercase();
        match name.as_str() {
            "EOH" => rec.clear(),
            "EOR" => {
                if !rec.is_empty() {
                    out.push(std::mem::take(&mut rec));
                }
            }
            _ => {
                let len: usize = parts.next().and_then(|l| l.trim().parse().ok()).unwrap_or(0);
                let end = (i + len).min(b.len());
                rec.insert(name, String::from_utf8_lossy(&b[i..end]).trim().to_string());
                i = end;
            }
        }
    }
    if !rec.is_empty() {
        out.push(rec);
    }
    out
}

/// A record from QRZ as a [`QrzQso`]; `None` if it has no call or date.
pub fn qso_from(rec: &HashMap<String, String>) -> Option<QrzQso> {
    let call = rec.get("CALL")?.trim().to_uppercase();
    let date = rec.get("QSO_DATE")?;
    if call.is_empty() || date.len() != 8 {
        return None;
    }
    let num = |s: &str, a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<i64>().ok());
    let time = rec.get("TIME_ON").map(String::as_str).unwrap_or("0000");
    let when = contacts::from_utc(
        num(date, 0, 4)?,
        num(date, 4, 6)?,
        num(date, 6, 8)?,
        num(time, 0, 2).unwrap_or(0),
        num(time, 2, 4).unwrap_or(0),
        num(time, 4, 6).unwrap_or(0),
    );
    let logid = rec.get("APP_QRZLOG_LOGID").and_then(|v| v.parse().ok()).unwrap_or(0);
    let get = |k: &str| rec.get(k).cloned().unwrap_or_default();
    Some(QrzQso { logid, call, name: get("NAME"), when, band: get("BAND"), mode: get("MODE"), comment: get("COMMENT") })
}

/// A call sign reduced to who it is: upper case, no `-R`/`-L` node suffix and no
/// `/P`, `/M` style ending.
pub fn norm(call: &str) -> String {
    let c = call.trim().to_uppercase();
    let c = c.strip_suffix("-R").or_else(|| c.strip_suffix("-L")).unwrap_or(&c);
    c.split('/').next().unwrap_or(c).to_string()
}

/// How often, and when last, you've worked a station.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Worked {
    pub count: u32,
    pub last: i64,
}

/// Everyone you've worked: contacts made here, plus the QRZ copy of the rest.
pub fn worked_index(qrz: &[QrzQso], local: &[Contact]) -> HashMap<String, Worked> {
    let mut map: HashMap<String, Worked> = HashMap::new();
    let mut add = |call: &str, when: i64| {
        let w = map.entry(norm(call)).or_insert(Worked { count: 0, last: 0 });
        w.count += 1;
        w.last = w.last.max(when);
    };
    let claimed = claimed_ids(local);
    for q in qrz.iter().filter(|q| !claimed.contains(&q.logid)) {
        add(&q.call, q.when);
    }
    for c in local.iter().filter(|c| c.is_qso()) {
        add(&c.call, c.start);
    }
    map
}

/// QRZ log ids that a contact made here already stands for (it was uploaded, or it
/// is an edit that will replace the old copy), so the QRZ copy isn't listed twice.
pub fn claimed_ids(local: &[Contact]) -> std::collections::HashSet<u64> {
    local
        .iter()
        .flat_map(|c| {
            let sent = match &c.qrz {
                contacts::Qrz::Sent(id) => id.parse::<u64>().ok(),
                _ => None,
            };
            [sent, c.replaces.as_deref().and_then(|r| r.parse().ok())]
        })
        .flatten()
        .collect()
}

/// QRZ log ids you chose to hide in this app (kept, so a re-sync doesn't bring them back).
pub fn load_hidden() -> std::collections::HashSet<u64> {
    std::fs::read_to_string(hidden_path()).map(|t| t.lines().filter_map(|l| l.trim().parse().ok()).collect()).unwrap_or_default()
}

pub fn save_hidden(ids: &std::collections::HashSet<u64>) {
    let mut v: Vec<_> = ids.iter().collect();
    v.sort();
    let text: String = v.iter().map(|i| format!("{i}\n")).collect();
    let _ = std::fs::write(hidden_path(), text);
}

fn hidden_path() -> PathBuf {
    dir().join("qrz-hidden.txt")
}

/// One line of the logbook tab: a contact from QRZ, or one made here.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Stable id: `L:` and the contact id for contacts made here, `Q:` and the log id for QRZ's.
    pub key: String,
    /// The QRZ log id, if QRZ has this contact.
    pub logid: Option<u64>,
    pub call: String,
    pub name: String,
    pub when: i64,
    pub band: String,
    pub mode: String,
    /// How long you were connected; only known for contacts made here.
    pub secs: Option<u64>,
    pub incoming: bool,
    pub state: EntryState,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EntryState {
    /// In your QRZ logbook.
    OnQrz,
    /// Made here and not (known to be) in QRZ; its upload status.
    Local(contacts::Qrz),
}

/// Everything in the logbook, newest first: the QRZ copy plus local contacts
/// that aren't in it.
pub fn entries(qrz: &[QrzQso], local: &[Contact]) -> Vec<Entry> {
    let claimed = claimed_ids(local);
    let mut out: Vec<Entry> = qrz
        .iter()
        .filter(|q| !claimed.contains(&q.logid))
        .map(|q| Entry {
            key: format!("Q:{}", q.logid),
            logid: Some(q.logid),
            call: q.call.clone(),
            name: q.name.clone(),
            when: q.when,
            band: q.band.clone(),
            mode: q.mode.clone(),
            secs: None,
            incoming: false,
            state: EntryState::OnQrz,
        })
        .collect();
    out.extend(local.iter().filter(|c| c.is_qso()).map(|c| Entry {
        key: format!("L:{}", c.id()),
        logid: match &c.qrz {
            contacts::Qrz::Sent(id) => id.parse().ok(),
            _ => None,
        },
        call: c.call.clone(),
        name: c.name.clone(),
        when: c.start,
        band: c.band.clone().unwrap_or_default(),
        mode: c.mode.clone().unwrap_or_default(),
        secs: Some(c.secs),
        incoming: c.incoming,
        state: EntryState::Local(c.qrz.clone()),
    }));
    out.sort_by(|a, b| b.when.cmp(&a.when));
    out
}

fn dir() -> PathBuf {
    crate::config::path().with_file_name("")
}

pub fn path() -> PathBuf {
    dir().join("qrz-logbook.jsonl")
}

fn sync_path() -> PathBuf {
    dir().join("qrz-synced.txt")
}

pub fn load() -> Vec<QrzQso> {
    let Ok(text) = std::fs::read_to_string(path()) else { return Vec::new() };
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

pub fn save(all: &[QrzQso]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path().with_extension("jsonl.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        for q in all {
            writeln!(f, "{}", serde_json::to_string(q)?)?;
        }
    }
    std::fs::rename(tmp, path())
}

/// When the last successful sync finished (Unix seconds).
pub fn last_sync() -> Option<i64> {
    std::fs::read_to_string(sync_path()).ok()?.trim().parse().ok()
}

pub fn set_last_sync(t: i64) {
    let _ = std::fs::write(sync_path(), t.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_adif_with_header_types_and_byte_lengths() {
        let text = "QRZ export\n<ADIF_VER:5>3.1.4 <EOH>\n\
            <app_qrzlog_logid:9>130877825<call:5>K6ABC<qso_date:8>20231114<time_on:4>2213<band:3>20m<mode:2>FM<name:5>Jos\u{e9} <eor>\n\
            <CALL:4>W1AW<QSO_DATE:8>20240101<TIME_ON:6>010203<COMMENT:12:S>a<b>c & d=e!<EOR>";
        let recs = parse_adif(text);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0]["CALL"], "K6ABC");
        assert_eq!(recs[0]["NAME"], "Jos\u{e9}", "a two-byte character counts as two");
        assert_eq!(recs[1]["COMMENT"], "a<b>c & d=e!");
        let q = qso_from(&recs[0]).unwrap();
        assert_eq!((q.logid, q.when, q.band.as_str(), q.mode.as_str()), (130877825, 1_699_999_980, "20m", "FM"));
        assert_eq!(qso_from(&recs[1]).unwrap().when, 1_704_070_923); // 2024-01-01 01:02:03 UTC
        assert!(qso_from(&parse_adif("<band:3>80m<eor>")[0]).is_none(), "no call, no contact");
    }

    #[test]
    fn dates_round_trip() {
        for t in [0, 951_782_400, 1_699_999_980, 1_704_070_923, 4_102_444_800] {
            let (d, h) = contacts::utc(t);
            let n = |s: &str, a, b| s[a..b].parse::<i64>().unwrap();
            assert_eq!(contacts::from_utc(n(&d, 0, 4), n(&d, 4, 6), n(&d, 6, 8), n(&h, 0, 2), n(&h, 2, 4), n(&h, 4, 6)), t);
        }
    }

    #[test]
    fn entries_merge_and_sort_newest_first() {
        let q = |logid, when| QrzQso { logid, call: "K6ABC".into(), name: String::new(), when, band: "2m".into(), mode: "FM".into(), comment: String::new() };
        let local = |start, state| Contact { call: "W1AW".into(), name: String::new(), node: None, start, secs: 90, incoming: false, qrz: state, band: None, mode: None, replaces: None, echolink: true };
        let mut blip = local(5, contacts::Qrz::Skipped);
        blip.secs = 3; // too short to be a contact
        let list = entries(&[q(1, 100), q(2, 500)], &[local(300, contacts::Qrz::Pending), local(500, contacts::Qrz::Sent("2".into())), blip]);
        assert_eq!(list.iter().map(|e| e.when).collect::<Vec<_>>(), [500, 300, 100], "the copy in QRZ isn't listed twice; non-contacts are left out");
        assert_eq!(list[1].state, EntryState::Local(contacts::Qrz::Pending));
        assert_eq!(list[1].secs, Some(90));
        assert_eq!((list[0].key.as_str(), list[0].logid), ("L:500/W1AW", Some(2)), "the local entry stands for QRZ's record 2");
        assert_eq!((list[2].key.as_str(), list[2].logid), ("Q:1", Some(1)));
    }

    #[test]
    fn an_edit_that_replaces_a_qrz_record_hides_the_old_copy_meanwhile() {
        let q = QrzQso { logid: 7, call: "K6ABC".into(), name: String::new(), when: 100, band: "2m".into(), mode: "FM".into(), comment: String::new() };
        let mut c = Contact { call: "K6ABC".into(), name: "fixed".into(), node: None, start: 100, secs: 60, incoming: false, qrz: contacts::Qrz::Pending, band: None, mode: None, replaces: Some("7".into()), echolink: true };
        assert_eq!(entries(&[q.clone()], &[c.clone()]).len(), 1, "the edited contact only; the old QRZ copy is not listed too");
        assert_eq!(worked_index(&[q.clone()], &[c.clone()])["K6ABC"].count, 1);
        c.replaces = None;
        assert_eq!(entries(&[q], &[c]).len(), 2, "once the old copy is no longer replaced it shows again");
    }

    #[test]
    fn call_normalizing() {
        assert_eq!(norm("w6xyz-r"), "W6XYZ");
        assert_eq!(norm("K6ABC/P"), "K6ABC");
        assert_eq!(norm("W1AW-L"), "W1AW");
    }

    #[test]
    fn worked_before_counts_both_sources_without_double_counting() {
        let q = |logid, call: &str, when| QrzQso { logid, call: call.into(), name: String::new(), when, band: String::new(), mode: String::new(), comment: String::new() };
        let qrz = [q(1, "K6ABC", 100), q(2, "K6ABC", 300), q(3, "W1AW", 50)];
        let mine = |call: &str, start, state| Contact { call: call.into(), name: String::new(), node: None, start, secs: 60, incoming: false, qrz: state, band: None, mode: None, replaces: None, echolink: true };
        let local = [
            mine("K6ABC", 300, contacts::Qrz::Sent("2".into())), // already in the QRZ copy
            mine("K6ABC-R", 900, contacts::Qrz::Pending),        // new, not on QRZ yet
            mine("*ECHOTEST*", 950, contacts::Qrz::Skipped),
        ];
        let w = worked_index(&qrz, &local);
        assert_eq!(w["K6ABC"], Worked { count: 3, last: 900 });
        assert_eq!(w["W1AW"], Worked { count: 1, last: 50 });
        assert!(!w.contains_key("*ECHOTEST*"));
    }
}
