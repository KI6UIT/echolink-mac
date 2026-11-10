//! Upload contacts to a QRZ logbook through its API
//! (<https://www.qrz.com/docs/logbook/QRZLogbookAPI.html>).
//!
//! QRZ gives each logbook its own access key, found on the QRZ website under the
//! logbook's settings. There is no way for an app to fetch it, so the user pastes it in.

use crate::logbook::{self, QrzQso};
use std::collections::BTreeMap;
use std::time::Duration;

pub const ENDPOINT: &str = "https://logbook.qrz.com/api";
/// Where the user finds (or creates) their logbook and its key.
pub const LOGBOOK_URL: &str = "https://logbook.qrz.com/";

/// QRZ requires an identifiable user agent: application name and version.
fn user_agent() -> String {
    format!("EchoLinkMac/{}", env!("CARGO_PKG_VERSION"))
}

pub struct Client {
    endpoint: String,
}

impl Default for Client {
    fn default() -> Self {
        Client { endpoint: ENDPOINT.into() }
    }
}

impl Client {
    #[cfg(test)]
    fn at(endpoint: &str) -> Client {
        Client { endpoint: endpoint.into() }
    }

    fn call(&self, key: &str, action: &str, extra: &[(&str, &str)]) -> Result<BTreeMap<String, String>, String> {
        let mut form = vec![("KEY", key.trim()), ("ACTION", action)];
        form.extend_from_slice(extra);
        let resp = ureq::post(&self.endpoint)
            .set("User-Agent", &user_agent())
            .timeout(Duration::from_secs(15))
            .send_form(&form);
        let body = match resp {
            Ok(r) => r.into_string().map_err(|e| format!("couldn't read QRZ's answer: {e}"))?,
            Err(ureq::Error::Status(code, _)) => return Err(format!("QRZ answered HTTP {code}")),
            Err(e) => return Err(format!("couldn't reach QRZ: {e}")),
        };
        let answer = parse(&body);
        // For troubleshooting: what QRZ said (never the key).
        let get = |k: &str| answer.get(k).map(String::as_str).unwrap_or("-");
        crate::log!(
            "qrz {action}: RESULT={} COUNT={} REASON={:?}; {} bytes, fields {:?}; starts {:?}",
            get("RESULT"),
            get("COUNT"),
            get("REASON"),
            body.len(),
            answer.keys().collect::<Vec<_>>(),
            body.chars().take(160).collect::<String>()
        );
        Ok(answer)
    }

    /// Put one ADIF record in the logbook. Returns the log id; a duplicate
    /// counts as success because the contact is already there.
    pub fn insert(&self, key: &str, adif: &str) -> Result<String, String> {
        let r = self.call(key, "INSERT", &[("ADIF", adif)])?;
        match r.get("RESULT").map(String::as_str) {
            Some("OK") | Some("REPLACE") => Ok(r.get("LOGID").or(r.get("LOGIDS")).cloned().unwrap_or_default()),
            Some("AUTH") => Err(reason(&r, "QRZ didn't accept the API key")),
            Some("FAIL") if reason(&r, "").to_lowercase().contains("duplicate") => Ok("duplicate".into()),
            _ => Err(reason(&r, "QRZ didn't accept the contact")),
        }
    }

    /// Delete records from the QRZ logbook. This is permanent: QRZ has no undo.
    /// Records QRZ no longer has count as deleted.
    pub fn delete(&self, key: &str, logids: &[u64]) -> Result<(), String> {
        let list = logids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let r = self.call(key, "DELETE", &[("LOGIDS", &list)])?;
        match r.get("RESULT").map(String::as_str) {
            Some("OK") => Ok(()),
            Some("AUTH") => Err(reason(&r, "QRZ didn't accept the API key")),
            // PARTIAL: what it didn't delete it didn't find, so everything asked for is gone.
            Some("PARTIAL") => Ok(()),
            // FAIL: none were found. Fine if that's the whole reason (already gone), an error otherwise.
            Some("FAIL") => {
                let missing: Vec<&str> = r.get("LOGIDS").map(|l| l.split(',').map(str::trim).collect()).unwrap_or_default();
                let all_missing = !missing.is_empty() && logids.iter().all(|id| missing.contains(&id.to_string().as_str()));
                let not_found = reason(&r, "").to_lowercase().contains("not found");
                if all_missing || not_found { Ok(()) } else { Err(reason(&r, "QRZ couldn't delete it")) }
            }
            _ => Err(reason(&r, "QRZ couldn't delete it")),
        }
    }

    /// Download the logbook from `after_logid` on, `page` records at a time, calling
    /// `on_page` with each batch. Returns how many contacts came down.
    pub fn sync(&self, key: &str, after_logid: u64, page: usize, mut on_page: impl FnMut(Vec<QrzQso>)) -> Result<usize, String> {
        let mut after = after_logid;
        let mut total = 0;
        for _ in 0..1000 {
            let option = format!("MAX:{page},AFTERLOGID:{after}");
            let r = self.call(key, "FETCH", &[("OPTION", &option)])?;
            match r.get("RESULT").map(String::as_str) {
                Some("OK") => {}
                // Nothing newer than what we have isn't a failure.
                Some("FAIL") if is_empty_answer(&r) => break,
                Some("AUTH") => return Err(reason(&r, "QRZ didn't accept the API key")),
                _ => return Err(reason(&r, "QRZ couldn't send the logbook")),
            }
            let records = logbook::parse_adif(r.get("ADIF").map(String::as_str).unwrap_or(""));
            let highest = records
                .iter()
                .filter_map(|rec| rec.get("APP_QRZLOG_LOGID").and_then(|v| v.parse::<u64>().ok()))
                .max()
                .unwrap_or(0);
            let qsos: Vec<QrzQso> = records.iter().filter_map(logbook::qso_from).collect();
            total += qsos.len();
            on_page(qsos);
            if records.len() < page {
                break; // a short page is the last one
            }
            if highest == 0 {
                return Err("QRZ sent more contacts but no log ids, so I can't ask for the rest".into());
            }
            after = highest + 1;
        }
        Ok(total)
    }

    /// Check that a key works. Returns something to show the user.
    pub fn status(&self, key: &str) -> Result<String, String> {
        let r = self.call(key, "STATUS", &[])?;
        match r.get("RESULT").map(String::as_str) {
            Some("OK") => {
                let who = ["CALLSIGN", "BOOKNAME", "BOOK_NAME", "OWNER"].iter().find_map(|k| r.get(*k)).cloned();
                let count = r.get("COUNT").cloned();
                Ok(match (who, count) {
                    (Some(w), Some(c)) => format!("{w} ({c} QSOs)"),
                    (Some(w), None) => w,
                    (None, Some(c)) => format!("{c} QSOs in the logbook"),
                    (None, None) => "key accepted".into(),
                })
            }
            _ => Err(reason(&r, "QRZ didn't accept the API key")),
        }
    }
}

/// QRZ answers a fetch that matches nothing with a FAIL that says so.
fn is_empty_answer(r: &BTreeMap<String, String>) -> bool {
    let why = r.get("REASON").map(|s| s.to_lowercase()).unwrap_or_default();
    r.get("COUNT").is_some_and(|c| c == "0") || ["no qso", "no record", "no match"].iter().any(|k| why.contains(k))
}

fn reason(r: &BTreeMap<String, String>, default: &str) -> String {
    r.get("REASON").cloned().unwrap_or_else(|| default.to_string())
}

/// `NAME=value&NAME=value`, with the values URL-decoded.
///
/// The ADIF in a fetch answer is the exception. QRZ sends it HTML-escaped
/// (`&lt;call:5&gt;K6ABC`), so it is full of `&` and can't be split like the rest;
/// it is cut out first and unescaped on its own.
pub fn parse(body: &str) -> BTreeMap<String, String> {
    let body = body.trim();
    let mut map = BTreeMap::new();
    let mut rest = body.to_string();
    let at = if body.starts_with("ADIF=") { Some(0) } else { body.find("&ADIF=").map(|p| p + 1) };
    if let Some(at) = at {
        let value = &body[at + 5..];
        // The value runs to the next real field, or the end.
        let end = ["&RESULT=", "&COUNT=", "&LOGIDS=", "&LOGID=", "&REASON=", "&DATA="]
            .iter()
            .filter_map(|k| value.find(k))
            .min()
            .unwrap_or(value.len());
        map.insert("ADIF".to_string(), adif_text(&value[..end]));
        rest = format!("{}{}", &body[..at], &value[end..]);
    }
    for (k, v) in rest.split('&').filter_map(|kv| kv.split_once('=')) {
        map.insert(k.trim().to_uppercase(), decode(v));
    }
    map
}

/// ADIF text as sent: HTML-escaped (what QRZ does) or percent-encoded.
fn adif_text(v: &str) -> String {
    if v.contains('<') || v.contains("&lt;") {
        unescape_html(v)
    } else {
        decode(v)
    }
}

pub(crate) fn unescape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let decoded = rest.find(';').filter(|&e| e <= 8).and_then(|e| {
            let ch = match &rest[1..e] {
                "lt" => '<',
                "gt" => '>',
                "amp" => '&',
                "quot" => '"',
                "apos" => '\'',
                "nbsp" => ' ',
                n if n.starts_with('#') => {
                    let code = match n[1..].strip_prefix(['x', 'X']) {
                        Some(h) => u32::from_str_radix(h, 16).ok()?,
                        None => n[1..].parse().ok()?,
                    };
                    char::from_u32(code)?
                }
                _ => return None,
            };
            Some((ch, e))
        });
        match decoded {
            Some((ch, e)) => {
                out.push(ch);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A stand-in for QRZ: answers one request with `body` and hands back what it was sent.
    fn fake_qrz(body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api", l.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = s.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req).to_string();
                if let Some(split) = text.find("\r\n\r\n") {
                    let len = text[..split]
                        .lines()
                        .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                        .unwrap_or(0);
                    if req.len() >= split + 4 + len {
                        break;
                    }
                }
            }
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            String::from_utf8_lossy(&req).to_string()
        });
        (url, h)
    }

    #[test]
    fn parses_answers() {
        let r = parse("RESULT=OK&LOGID=130877825&COUNT=1\n");
        assert_eq!((r["RESULT"].as_str(), r["LOGID"].as_str()), ("OK", "130877825"));
        assert_eq!(parse("RESULT=FAIL&REASON=Unable+to+add+QSO%3A+missing+band")["REASON"], "Unable to add QSO: missing band");
    }

    #[test]
    fn insert_sends_key_action_adif_and_a_user_agent() {
        let (url, server) = fake_qrz("RESULT=OK&LOGID=42&COUNT=1");
        let id = Client::at(&url).insert("ABCD-0A0B-1C1D-2E2F", "<CALL:5>K6ABC <EOR>").unwrap();
        assert_eq!(id, "42");
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /api"), "{req}");
        assert!(req.to_lowercase().contains(&format!("user-agent: echolinkmac/{}", env!("CARGO_PKG_VERSION"))), "{req}");
        assert!(req.contains("KEY=ABCD-0A0B-1C1D-2E2F") && req.contains("ACTION=INSERT"), "{req}");
        assert!(req.contains("ADIF=%3CCALL%3A5%3EK6ABC"), "{req}");
    }

    #[test]
    fn failures_come_back_with_qrz_reason() {
        let (url, _s) = fake_qrz("RESULT=FAIL&REASON=Unable+to+add+QSO+to+database%3A+invalid+band");
        assert_eq!(Client::at(&url).insert("K", "x").unwrap_err(), "Unable to add QSO to database: invalid band");
        let (url, _s) = fake_qrz("RESULT=AUTH&REASON=invalid+api+key");
        assert_eq!(Client::at(&url).insert("K", "x").unwrap_err(), "invalid api key");
        // Already there is fine.
        let (url, _s) = fake_qrz("RESULT=FAIL&REASON=Unable+to+add+QSO+to+database%3A+duplicate");
        assert_eq!(Client::at(&url).insert("K", "x").unwrap(), "duplicate");
    }

    #[test]
    fn delete_sends_the_ids_and_treats_already_gone_as_done() {
        let (url, server) = fake_qrz("RESULT=OK&COUNT=2");
        Client::at(&url).delete("KEY", &[11, 12]).unwrap();
        let req = server.join().unwrap();
        assert!(req.contains("ACTION=DELETE") && req.contains("LOGIDS=11%2C12"), "{req}");
        let (url, _s) = fake_qrz("RESULT=FAIL&REASON=none+of+the+logids+were+found&LOGIDS=11");
        Client::at(&url).delete("KEY", &[11]).unwrap();
        let (url, _s) = fake_qrz("RESULT=AUTH&REASON=invalid+api+key");
        assert_eq!(Client::at(&url).delete("KEY", &[11]).unwrap_err(), "invalid api key");
        let (url, _s) = fake_qrz("RESULT=PARTIAL&LOGIDS=12&COUNT=1");
        Client::at(&url).delete("KEY", &[11, 12]).unwrap();
        let (url, _s) = fake_qrz("RESULT=FAIL&REASON=a+subscription+is+required");
        assert_eq!(Client::at(&url).delete("KEY", &[11]).unwrap_err(), "a subscription is required");
    }

    #[test]
    fn status_reports_who_it_is() {
        let (url, _s) = fake_qrz("RESULT=OK&CALLSIGN=KI6UIT&COUNT=12");
        assert_eq!(Client::at(&url).status("K").unwrap(), "KI6UIT (12 QSOs)");
        let (url, _s) = fake_qrz("RESULT=FAIL&REASON=invalid+api+key");
        assert_eq!(Client::at(&url).status("K").unwrap_err(), "invalid api key");
    }

    /// Answers each request with the next body in `bodies`; returns what it was sent.
    fn fake_qrz_pages(bodies: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api", l.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for body in bodies {
                let (mut s, _) = l.accept().unwrap();
                let mut req = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = s.read(&mut buf).unwrap();
                    req.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&req).to_string();
                    if let Some(split) = text.find("\r\n\r\n") {
                        let len = text[..split]
                            .lines()
                            .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                            .unwrap_or(0);
                        if req.len() >= split + 4 + len {
                            break;
                        }
                    }
                }
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                seen.push(String::from_utf8_lossy(&req).to_string());
            }
            seen
        });
        (url, h)
    }

    fn encode(s: &str) -> String {
        s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
    }

    fn adif_of(logids: &[u64]) -> String {
        let adif: String = logids
            .iter()
            .map(|id| format!("<app_qrzlog_logid:{}>{id}<call:5>K6A{:02}<qso_date:8>20240101<time_on:4>1200<band:3>20m<mode:2>FM<eor>\n", id.to_string().len(), id % 100))
            .collect();
        format!("RESULT=OK&COUNT={}&ADIF={}", logids.len(), encode(&adif))
    }

    #[test]
    fn reads_qrz_html_escaped_adif() {
        // The shape QRZ really sends: ADIF escaped with entities, fields in any order.
        let body = "COUNT=2&RESULT=OK&ADIF=&lt;band:4&gt;70cm\n&lt;call:5&gt;KM6PS\n&lt;comment:9&gt;R&amp;D + 73!\n&lt;app_qrzlog_logid:3&gt;123\n&lt;qso_date:8&gt;20240101\n&lt;eor&gt;\n\
            &lt;call:5&gt;W1AW \n&lt;app_qrzlog_logid:3&gt;124\n&lt;qso_date:8&gt;20240102\n&lt;eor&gt;\n";
        let r = parse(body);
        assert_eq!((r["RESULT"].as_str(), r["COUNT"].as_str()), ("OK", "2"));
        let recs = logbook::parse_adif(&r["ADIF"]);
        assert_eq!(recs.len(), 2, "{:?}", r["ADIF"]);
        assert_eq!(recs[0]["COMMENT"], "R&D + 73!", "ampersand and plus survive");
        assert_eq!(recs[1]["APP_QRZLOG_LOGID"], "124");
        // Fields after the ADIF are still found.
        assert_eq!(parse("ADIF=&lt;call:1&gt;A&lt;eor&gt;&RESULT=OK&COUNT=1")["COUNT"], "1");
        // Unknown entities are left alone.
        assert_eq!(parse("RESULT=OK&ADIF=a &amp; b &copy; c &lt;")["ADIF"], "a & b &copy; c <");
    }

    #[test]
    fn sync_pages_through_the_logbook() {
        // Pages of 2: a full page, then a short one that ends it.
        let (url, server) = fake_qrz_pages(vec![adif_of(&[10, 11]), adif_of(&[15])]);
        let mut got = Vec::new();
        let n = Client::at(&url).sync("KEY", 0, 2, |page| got.extend(page)).unwrap();
        assert_eq!(n, 3);
        assert_eq!(got.iter().map(|q| q.logid).collect::<Vec<_>>(), [10, 11, 15]);
        let seen = server.join().unwrap();
        assert!(seen[0].contains("ACTION=FETCH") && seen[0].contains("MAX%3A2%2CAFTERLOGID%3A0"), "{}", seen[0]);
        assert!(seen[1].contains("AFTERLOGID%3A12"), "second page starts after the highest id: {}", seen[1]);
    }

    #[test]
    fn sync_treats_nothing_new_as_success_and_real_failures_as_errors() {
        let (url, _s) = fake_qrz_pages(vec!["RESULT=FAIL&REASON=no+QSOs+found&COUNT=0".into()]);
        assert_eq!(Client::at(&url).sync("K", 99, 250, |_| panic!("no page expected")).unwrap(), 0);
        let (url, _s) = fake_qrz_pages(vec!["RESULT=AUTH&REASON=invalid+api+key".into()]);
        assert_eq!(Client::at(&url).sync("K", 0, 250, |_| {}).unwrap_err(), "invalid api key");
    }

    #[test]
    fn unreachable_server_is_a_clear_error() {
        let e = Client::at("http://127.0.0.1:1/api").insert("K", "x").unwrap_err();
        assert!(e.starts_with("couldn't reach QRZ"), "{e}");
    }
}
