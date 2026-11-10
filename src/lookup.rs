//! Callsign lookups from QRZ's XML Data service
//! (<https://www.qrz.com/XML/current_spec.html>): what is on someone's QRZ page.
//!
//! This is a different service from the logbook, and signs in with your QRZ
//! username and password. QRZ shows full details only to subscribers; others get a
//! reduced record and a message saying so.

use crate::qrz::unescape_html;
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const BASE: &str = "https://xmldata.qrz.com/xml/current/";
const MAX_IMAGE: u64 = 4 * 1024 * 1024;
const MAX_BIO_CHARS: usize = 8000;

fn agent() -> String {
    format!("EchoLinkMac-{}", env!("CARGO_PKG_VERSION"))
}

/// What QRZ knows about a station.
#[derive(Clone, Debug, Default)]
pub struct Page {
    /// Callsign fields as QRZ names them: `fname`, `name`, `addr2`, `grid`, ...
    pub fields: BTreeMap<String, String>,
    /// The biography as plain text, if there is one.
    pub bio: Option<String>,
    /// The profile photo, undecoded.
    pub image: Option<Vec<u8>>,
    /// A notice from QRZ, e.g. that a subscription is needed for the full record.
    pub notice: Option<String>,
}

impl Page {
    pub fn get(&self, k: &str) -> &str {
        self.fields.get(k).map(String::as_str).unwrap_or("")
    }

    /// The name as QRZ shows it.
    pub fn full_name(&self) -> String {
        let f = self.get("name_fmt");
        if !f.is_empty() {
            return f.to_string();
        }
        format!("{} {}", self.get("fname"), self.get("name")).trim().to_string()
    }

    /// The mailing address, one line per row.
    pub fn address(&self) -> Vec<String> {
        let city = [self.get("addr2"), self.get("state"), self.get("zip")].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
        [self.get("attn"), self.get("addr1"), city.as_str(), self.get("country")]
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }
}

/// Who you are signed in as, and the plan QRZ says you have.
#[derive(Clone, Debug)]
pub struct Signin {
    pub subscription: String,
}

/// A QRZ XML connection. The session key is shared (and reused) between lookups, as QRZ asks.
#[derive(Clone)]
pub struct Client {
    base: String,
    user: String,
    pass: String,
    session: Arc<Mutex<Option<String>>>,
}

impl Client {
    pub fn new(user: &str, pass: &str, session: Arc<Mutex<Option<String>>>) -> Client {
        Client { base: BASE.into(), user: user.trim().into(), pass: pass.into(), session }
    }

    #[cfg(test)]
    fn at(base: &str, session: Arc<Mutex<Option<String>>>) -> Client {
        Client { base: base.into(), user: "KI6UIT".into(), pass: "secret".into(), session }
    }

    /// Passwords go in the body of a POST, never the address.
    fn post(&self, form: &[(&str, &str)]) -> Result<String, String> {
        ureq::post(&self.base)
            .set("User-Agent", &agent())
            .timeout(Duration::from_secs(15))
            .send_form(form)
            .map_err(|e| match e {
                ureq::Error::Status(code, _) => format!("QRZ answered HTTP {code}"),
                e => format!("couldn't reach QRZ: {e}"),
            })?
            .into_string()
            .map_err(|e| format!("couldn't read QRZ's answer: {e}"))
    }

    /// Sign in and remember the session key.
    pub fn login(&self) -> Result<Signin, String> {
        if self.user.is_empty() || self.pass.is_empty() {
            return Err("Add your QRZ username and password in Settings.".into());
        }
        let xml = self.post(&[("username", &self.user), ("password", &self.pass), ("agent", &agent())])?;
        if let Some(e) = tag(&xml, "Error") {
            *self.session.lock().unwrap() = None;
            return Err(e);
        }
        let Some(key) = tag(&xml, "Key") else { return Err("QRZ didn't give a session key".into()) };
        *self.session.lock().unwrap() = Some(key);
        Ok(Signin { subscription: tag(&xml, "SubExp").unwrap_or_default() })
    }

    fn key(&self) -> Result<String, String> {
        if let Some(k) = self.session.lock().unwrap().clone() {
            return Ok(k);
        }
        self.login()?;
        self.session.lock().unwrap().clone().ok_or_else(|| "QRZ didn't give a session key".into())
    }

    /// One request that needs a session; signs in again once if the session has lapsed.
    fn with_session(&self, extra: (&str, &str)) -> Result<String, String> {
        for attempt in 0..2 {
            let key = self.key()?;
            let xml = self.post(&[("s", &key), extra])?;
            let lapsed = tag(&xml, "Session").is_some() && tag(&xml, "Key").is_none() && tag(&xml, "Callsign").is_none() && xml.contains("<Error>");
            let timed_out = lapsed && tag(&xml, "Error").is_some_and(|e| e.to_lowercase().contains("session") || e.to_lowercase().contains("invalid"));
            if timed_out && attempt == 0 {
                *self.session.lock().unwrap() = None;
                continue;
            }
            return Ok(xml);
        }
        Err("QRZ keeps refusing the session".into())
    }

    /// Everything on a station's QRZ page: details, biography and photo.
    pub fn lookup(&self, call: &str) -> Result<Page, String> {
        let call = lookup_call(call);
        let xml = self.with_session(("callsign", &call))?;
        let Some(block) = tag_raw(&xml, "Callsign") else {
            return Err(tag(&xml, "Error").unwrap_or_else(|| format!("{call} isn't on QRZ")));
        };
        let mut page = Page { notice: tag(&xml, "Message"), ..Default::default() };
        for name in FIELDS {
            if let Some(v) = tag(block, name) {
                if !v.is_empty() {
                    page.fields.insert(name.to_string(), v);
                }
            }
        }
        if page.fields.contains_key("bio") {
            page.bio = self.with_session(("html", &call)).ok().map(|h| html_to_text(&h)).filter(|t| !t.is_empty());
        }
        if let Some(url) = page.fields.get("image") {
            page.image = fetch_image(url);
        }
        Ok(page)
    }
}

/// The part of a directory call QRZ knows: no `-R` / `-L` node suffix.
pub fn lookup_call(call: &str) -> String {
    let c = call.trim().to_uppercase();
    c.strip_suffix("-R").or_else(|| c.strip_suffix("-L")).unwrap_or(&c).to_string()
}

/// True if `s` could be an amateur call sign (letters, digits, `/`, at least one digit).
pub fn looks_like_call(s: &str) -> bool {
    let s = s.trim();
    (3..=12).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '/') && s.chars().any(|c| c.is_ascii_digit()) && s.chars().any(|c| c.is_ascii_alphabetic())
}

const FIELDS: [&str; 46] = [
    "call", "aliases", "xref", "dxcc", "fname", "name", "nickname", "name_fmt", "attn", "addr1", "addr2", "state", "zip", "country", "lat", "lon", "grid", "county",
    "land", "efdate", "expdate", "p_call", "class", "codes", "qslmgr", "email", "url", "u_views", "bio", "biodate", "image", "moddate", "TimeZone", "eqsl", "mqsl",
    "lotw", "cqzone", "ituzone", "iota", "born", "geoloc", "user", "AreaCode", "fips", "MSA", "serial",
];

/// The text inside `<name>…</name>`, exactly as sent.
fn tag_raw<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = xml.find(&format!("<{name}>"))? + name.len() + 2;
    let close = xml[open..].find(&format!("</{name}>"))?;
    Some(&xml[open..open + close])
}

/// The trimmed, unescaped text inside `<name>…</name>`.
fn tag(xml: &str, name: &str) -> Option<String> {
    tag_raw(xml, name).map(|v| unescape_html(v.trim()))
}

/// Only fetch pictures from QRZ's own https hosts, so a profile can't point the
/// app at something on your network.
pub fn image_allowed(url: &str) -> bool {
    let Some(rest) = url.trim().strip_prefix("https://") else { return false };
    let host = rest.split(['/', ':', '?', '#']).next().unwrap_or("").to_lowercase();
    !host.contains('@') && (host == "qrz.com" || host.ends_with(".qrz.com"))
}

fn fetch_image(url: &str) -> Option<Vec<u8>> {
    if !image_allowed(url) {
        return None;
    }
    let resp = ureq::get(url.trim()).set("User-Agent", &agent()).timeout(Duration::from_secs(10)).call().ok()?;
    let mut bytes = Vec::new();
    resp.into_reader().take(MAX_IMAGE).read_to_end(&mut bytes).ok()?;
    (!bytes.is_empty()).then_some(bytes)
}

/// A web page reduced to readable text.
pub fn html_to_text(html: &str) -> String {
    let mut s = html.to_string();
    // Drop whole blocks that aren't text.
    for tagname in ["style", "script", "head"] {
        loop {
            let lower = s.to_lowercase();
            let Some(a) = lower.find(&format!("<{tagname}")) else { break };
            let Some(b) = lower[a..].find(&format!("</{tagname}>")) else {
                s.truncate(a);
                break;
            };
            s.replace_range(a..a + b + tagname.len() + 3, "");
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '<' {
            out.push(c);
            continue;
        }
        let mut name = String::new();
        for t in chars.by_ref() {
            if t == '>' {
                break;
            }
            name.push(t);
        }
        let n = name.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("").to_lowercase();
        if n == "br" {
            out.push('\n');
        } else if matches!(n.as_str(), "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "table") {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n'); // a new line, but not an empty one
            }
        } else if matches!(n.as_str(), "td" | "th") {
            out.push(' ');
        }
    }
    let text = unescape_html(&out).replace('\u{a0}', " ");
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() && lines.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    let text = lines.join("\n").trim().to_string();
    if text.chars().count() > MAX_BIO_CHARS {
        format!("{}…", text.chars().take(MAX_BIO_CHARS).collect::<String>())
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    /// A stand-in for QRZ: answers each request with the next body and returns what it was sent.
    fn fake(bodies: Vec<&'static str>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/xml/current/", l.local_addr().unwrap());
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

    const KEY_REPLY: &str = r#"<?xml version="1.0" ?><QRZDatabase version="1.34"><Session><Key>abc123</Key><Count>5</Count><SubExp>Wed Jan 1 12:34:03 2031</SubExp></Session></QRZDatabase>"#;
    const CALL_REPLY: &str = r#"<?xml version="1.0" ?><QRZDatabase version="1.34"><Callsign>
        <call>AA7BQ</call> <fname>FRED L</fname> <name>LLOYD &amp; SON</name> <addr1>8711 E PINNACLE PEAK RD 193</addr1>
        <addr2>SCOTTSDALE</addr2> <state>AZ</state> <zip>85255</zip> <country>United States</country> <grid>DM32af</grid>
        <class>E</class> <email> [email protected] </email> <url>https://www.qrz.com/db/aa7bq</url> <bio>3937/2003-11-04</bio>
        </Callsign><Session><Key>abc123</Key></Session></QRZDatabase>"#;

    #[test]
    fn sign_in_sends_credentials_in_the_body_with_an_agent() {
        let (url, server) = fake(vec![KEY_REPLY]);
        let session = Arc::new(Mutex::new(None));
        let s = Client::at(&url, session.clone()).login().unwrap();
        assert_eq!(s.subscription, "Wed Jan 1 12:34:03 2031");
        assert_eq!(session.lock().unwrap().as_deref(), Some("abc123"));
        let req = server.join().unwrap().remove(0);
        assert!(req.starts_with("POST /xml/current/ "), "credentials must not be in the URL: {req}");
        assert!(req.contains("username=KI6UIT") && req.contains("password=secret") && req.contains("agent=EchoLinkMac-"), "{req}");
    }

    #[test]
    fn bad_login_and_unknown_call_say_why() {
        let (url, _s) = fake(vec![r#"<QRZDatabase><Session><Error>Username/password incorrect </Error></Session></QRZDatabase>"#]);
        assert_eq!(Client::at(&url, Default::default()).login().unwrap_err(), "Username/password incorrect");
        let (url, _s) = fake(vec![r#"<QRZDatabase><Session><Error>Not found: ZZ9ZZZ</Error><Key>abc123</Key></Session></QRZDatabase>"#]);
        let c = Client::at(&url, Arc::new(Mutex::new(Some("abc123".into()))));
        assert_eq!(c.lookup("zz9zzz").unwrap_err(), "Not found: ZZ9ZZZ");
    }

    #[test]
    fn lookup_reads_the_page_and_reuses_the_session() {
        let (url, server) = fake(vec![CALL_REPLY, "<html><body><h1>About me</h1><p>Ham since <b>1975</b>.</p></body></html>"]);
        let c = Client::at(&url, Arc::new(Mutex::new(Some("abc123".into()))));
        let page = c.lookup("aa7bq-r").unwrap(); // a repeater's -R suffix is dropped
        assert_eq!(page.full_name(), "FRED L LLOYD & SON");
        assert_eq!(page.address(), ["8711 E PINNACLE PEAK RD 193", "SCOTTSDALE AZ 85255", "United States"]);
        assert_eq!((page.get("grid"), page.get("class")), ("DM32af", "E"));
        assert_eq!(page.get("email"), "[email protected]");
        assert_eq!(page.bio.as_deref(), Some("About me\nHam since 1975."));
        let seen = server.join().unwrap();
        assert!(seen[0].contains("s=abc123") && seen[0].contains("callsign=AA7BQ"), "{}", seen[0]);
        assert!(seen[1].contains("html=AA7BQ"), "the bio is a separate request: {}", seen[1]);
    }

    #[test]
    fn an_expired_session_signs_in_again_once() {
        let (url, server) = fake(vec![
            r#"<QRZDatabase><Session><Error>Session Timeout</Error></Session></QRZDatabase>"#,
            KEY_REPLY.replace("abc123", "fresh").leak(),
            r#"<QRZDatabase><Callsign><call>K6ABC</call><fname>PAT</fname></Callsign><Session><Key>fresh</Key></Session></QRZDatabase>"#,
        ]);
        let session = Arc::new(Mutex::new(Some("stale".into())));
        let page = Client::at(&url, session.clone()).lookup("K6ABC").unwrap();
        assert_eq!(page.get("fname"), "PAT");
        assert_eq!(session.lock().unwrap().as_deref(), Some("fresh"));
        let seen = server.join().unwrap();
        assert!(seen[0].contains("s=stale") && seen[1].contains("username=") && seen[2].contains("s=fresh"));
    }

    #[test]
    fn subscription_notice_is_kept() {
        let (url, _s) = fake(vec![r#"<QRZDatabase><Callsign><call>K6ABC</call></Callsign><Session><Key>k</Key><Message>A subscription is required to obtain the complete data</Message></Session></QRZDatabase>"#]);
        let page = Client::at(&url, Arc::new(Mutex::new(Some("k".into())))).lookup("K6ABC").unwrap();
        assert_eq!(page.notice.as_deref(), Some("A subscription is required to obtain the complete data"));
    }

    #[test]
    fn only_qrz_pictures_are_fetched() {
        assert!(image_allowed("https://files.qrz.com/q/aa7bq/aa7bq.jpg"));
        assert!(image_allowed("https://qrz.com/x.png"));
        for bad in ["http://files.qrz.com/a.jpg", "https://evil.example/a.jpg", "https://files.qrz.com.evil.example/a.jpg", "https://127.0.0.1/a.jpg", "https://qrz.com@evil.example/a.jpg", "file:///etc/passwd"] {
            assert!(!image_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn html_becomes_plain_text() {
        let t = html_to_text("<html><head><style>p{color:red}</style></head><body><script>alert(1)</script><p>Hello&nbsp;&amp; welcome</p><ul><li>one</li><li>two</li></ul><br><br><br>end</body></html>");
        assert_eq!(t, "Hello & welcome\none\ntwo\n\nend");
    }

    #[test]
    fn call_helpers() {
        assert_eq!(lookup_call("w6xyz-r"), "W6XYZ");
        assert_eq!(lookup_call("K6ABC/P"), "K6ABC/P");
        for ok in ["K6ABC", "w1aw", "VE7XYZ", "K6ABC/P", "9A1A"] {
            assert!(looks_like_call(ok), "{ok}");
        }
        for no in ["", "hello", "123", "ab", "pat smith", "K6ABC; drop", "this is far too long to be a call"] {
            assert!(!looks_like_call(no), "{no}");
        }
    }
}
