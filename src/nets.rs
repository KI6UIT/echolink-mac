//! Finding nets: a schedule of EchoLink nets, read from NetFinder (netfinder.radio),
//! a community-run directory of amateur radio nets.
//!
//! NetFinder has no API, so this reads its public web pages. To be a good guest it
//! identifies itself, reads nothing until the NETS tab is opened, keeps what it fetched
//! for a day, fetches pages one second apart, and every net links back to its NetFinder page.

use crate::qrz::unescape_html;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;

pub const SITE: &str = "https://netfinder.radio";
const LIST_PATH: &str = "/nets/modes/echolink";
/// The list isn't re-read more often than this unless you press REFRESH.
pub const MAX_AGE_SECS: i64 = 24 * 3600;
/// REFRESH waits at least this long after the last fetch.
pub const MIN_REFRESH_SECS: i64 = 600;
const PAGE_DELAY: Duration = Duration::from_millis(1000);
const MAX_PAGES: usize = 25;
pub const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

fn agent() -> String {
    format!("EchoLinkMac/{} (net list reader)", env!("CARGO_PKG_VERSION"))
}

/// One line of the list.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Net {
    /// The part of the address after `/nets/view/`.
    pub slug: String,
    pub name: String,
    /// Monday first, in UTC.
    pub days: [bool; 7],
    /// Repeater, DMR, AllStar, SKYWARN net, ...
    pub tags: Vec<String>,
    pub band: String,
    pub website: Option<String>,
}

/// A net's own page.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Detail {
    pub name: String,
    pub slots: Vec<Slot>,
    /// Organizer, State, Country, Band, ...
    pub fields: Vec<(String, String)>,
    /// The EchoLink node to call, as NetFinder gives it (e.g. `W4GSO-R`).
    pub echolink_node: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    /// `8:30 PM Sunday` and the time zone it was written in.
    pub local: String,
    pub timezone: String,
    /// `01:30 Monday`
    pub utc: String,
    /// `W4GSO Repeater 145.150 MHz`
    pub frequency: String,
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid pattern"))
}

/// Text with tags removed, entities decoded and spaces tidied.
fn text_of(html: &str) -> String {
    static TAG: OnceLock<Regex> = OnceLock::new();
    let plain = re(&TAG, r"(?s)<[^>]*>").replace_all(&html.replace("<br>", " "), " ").into_owned();
    unescape_html(&plain).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Nets on one page of the list, and the highest page number it links to.
pub fn parse_list(html: &str) -> (Vec<Net>, usize) {
    static ROW: OnceLock<Regex> = OnceLock::new();
    static NAME: OnceLock<Regex> = OnceLock::new();
    static DAY: OnceLock<Regex> = OnceLock::new();
    static BOX: OnceLock<Regex> = OnceLock::new();
    static SITE_LINK: OnceLock<Regex> = OnceLock::new();
    static PAGE: OnceLock<Regex> = OnceLock::new();

    let mut nets = Vec::new();
    for row in re(&ROW, r"(?s)<tr>(.*?)</tr>").captures_iter(html) {
        let row = &row[1];
        if row.contains("<th") {
            continue;
        }
        let Some(n) = re(&NAME, r#"(?s)<a href="/nets/view/([^"]+)">(.*?)</a>"#).captures(row) else { continue };
        let mut days = [false; 7];
        for d in re(&DAY, r#"class="feature-box (day|day-off)" title="(\w+)""#).captures_iter(row) {
            if let Some(i) = DAYS.iter().position(|&name| name == &d[2]) {
                days[i] = &d[1] == "day";
            }
        }
        let (mut tags, mut band) = (Vec::new(), String::new());
        for b in re(&BOX, r#"(?s)class="feature-box ([^"]*)"[^>]*>\s*([^<]*?)\s*</div>"#).captures_iter(row) {
            let (class, label) = (&b[1], unescape_html(b[2].trim()));
            if class.starts_with("day") || class.starts_with("website") || label.is_empty() {
                continue;
            }
            if class.contains("all-nets") {
                band = label;
            } else {
                tags.push(label);
            }
        }
        let website = re(&SITE_LINK, r#"<a href="([^"]+)"[^>]*><div class="feature-box website""#).captures(row).map(|w| unescape_html(&w[1]));
        nets.push(Net { slug: n[1].to_string(), name: text_of(&n[2]), days, tags, band, website });
    }
    let last = re(&PAGE, r"[?&]page=(\d+)").captures_iter(html).filter_map(|c| c[1].parse().ok()).max().unwrap_or(1);
    (nets, last)
}

/// A net's page.
pub fn parse_detail(html: &str) -> Detail {
    static H1: OnceLock<Regex> = OnceLock::new();
    static ROW: OnceLock<Regex> = OnceLock::new();
    static TZ: OnceLock<Regex> = OnceLock::new();
    static FIELD: OnceLock<Regex> = OnceLock::new();

    let name = re(&H1, r"(?s)<h1>(.*?)(?:&nbsp;|<span|</h1>)").captures(html).map(|c| text_of(&c[1])).unwrap_or_default();
    let mut slots = Vec::new();
    // The schedule is the first table on the page.
    if let Some(table) = html.split("<table").nth(1).and_then(|t| t.split("</table>").next()) {
        for row in re(&ROW, r"(?s)<tr>\s*<td>(.*?)</td>\s*<td[^>]*>(.*?)</td>\s*<td[^>]*>(.*?)</td>").captures_iter(table) {
            let timezone = re(&TZ, r#"<span class="timezone">(.*?)</span>"#).captures(&row[1]).map(|t| text_of(&t[1])).unwrap_or_default();
            let local = text_of(&re(&TZ, r#"<span class="timezone">(.*?)</span>"#).replace(&row[1], ""));
            slots.push(Slot { local, timezone, utc: text_of(&row[2]), frequency: text_of(&row[3]) });
        }
    }
    let mut fields = Vec::new();
    let mut echolink_node = None;
    for f in re(&FIELD, r"(?s)<p><strong>([^<]+)</strong><br>(.*?)</p>").captures_iter(html) {
        let (label, value) = (text_of(&f[1]), text_of(&f[2]));
        if value.is_empty() {
            continue;
        }
        if label.eq_ignore_ascii_case("EchoLink Node") {
            echolink_node = Some(value.clone());
        }
        fields.push((label, value));
    }
    Detail { name, slots, fields, echolink_node }
}

/// Whole list, page by page (one second apart). `progress` hears how many pages are done.
pub fn fetch_list(base: &str, mut progress: impl FnMut(usize, usize)) -> Result<Vec<Net>, String> {
    let mut all: Vec<Net> = Vec::new();
    let (first, last) = {
        let html = get(&format!("{base}{LIST_PATH}"))?;
        parse_list(&html)
    };
    all.extend(first);
    let last = last.min(MAX_PAGES);
    progress(1, last);
    for page in 2..=last {
        std::thread::sleep(PAGE_DELAY);
        let html = get(&format!("{base}{LIST_PATH}?page={page}"))?;
        all.extend(parse_list(&html).0);
        progress(page, last);
    }
    if all.is_empty() {
        return Err("NetFinder's page didn't look as expected, so no nets were read. Its layout may have changed.".into());
    }
    let mut seen = std::collections::HashSet::new();
    all.retain(|n| seen.insert(n.slug.clone()));
    Ok(all)
}

pub fn fetch_detail(base: &str, slug: &str) -> Result<Detail, String> {
    if !slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("odd net address".into());
    }
    let d = parse_detail(&get(&format!("{base}/nets/view/{slug}"))?);
    if d.name.is_empty() && d.slots.is_empty() {
        return Err("NetFinder's page didn't look as expected.".into());
    }
    Ok(d)
}

fn get(url: &str) -> Result<String, String> {
    ureq::get(url)
        .set("User-Agent", &agent())
        .timeout(Duration::from_secs(20))
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("NetFinder answered HTTP {code}"),
            e => format!("couldn't reach NetFinder: {e}"),
        })?
        .into_string()
        .map_err(|e| format!("couldn't read NetFinder's answer: {e}"))
}

/// Where a net's page is on the web (the slug is checked to be plain).
pub fn page_url(slug: &str) -> Option<String> {
    slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_').then(|| format!("{SITE}/nets/view/{slug}"))
}

// ---- the cache -------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Cache {
    fetched_at: i64,
    nets: Vec<Net>,
}

fn cache_path() -> std::path::PathBuf {
    crate::config::path().with_file_name("nets-cache.json")
}

pub fn load_cache() -> Option<(i64, Vec<Net>)> {
    let c: Cache = serde_json::from_str(&std::fs::read_to_string(cache_path()).ok()?).ok()?;
    Some((c.fetched_at, c.nets))
}

pub fn save_cache(fetched_at: i64, nets: &[Net]) {
    if let Ok(text) = serde_json::to_string(&Cache { fetched_at, nets: nets.to_vec() }) {
        let _ = std::fs::write(cache_path(), text);
    }
}

// ---- times ---------------------------------------------------------------------------

/// 0 = Monday, for a Unix time.
pub fn weekday(unix: i64) -> usize {
    ((unix.div_euclid(86400) + 3).rem_euclid(7)) as usize
}

/// This computer's offset from UTC in minutes (from `date`), or 0.
pub fn local_offset_minutes() -> i32 {
    let out = std::process::Command::new("date").arg("+%z").output().ok().and_then(|o| String::from_utf8(o.stdout).ok()).unwrap_or_default();
    let t = out.trim();
    if t.len() == 5 && (t.starts_with('+') || t.starts_with('-')) {
        if let (Ok(h), Ok(m)) = (t[1..3].parse::<i32>(), t[3..5].parse::<i32>()) {
            let v = h * 60 + m;
            return if t.starts_with('-') { -v } else { v };
        }
    }
    0
}

/// A UTC slot like `01:30 Monday` as it falls in your time zone: `Sun 8:30 PM`.
pub fn in_my_time(utc: &str, offset_min: i32) -> Option<String> {
    let mut parts = utc.split_whitespace();
    let (h, m) = parts.next()?.split_once(':')?;
    let name = parts.next()?;
    let day = DAYS.iter().position(|d| d.eq_ignore_ascii_case(name))? as i32;
    let total = day * 1440 + h.parse::<i32>().ok()? * 60 + m.parse::<i32>().ok()? + offset_min;
    let total = total.rem_euclid(7 * 1440);
    let (day, minute) = (total / 1440, total % 1440);
    let (h24, mi) = (minute / 60, minute % 60);
    let (h12, ampm) = match h24 {
        0 => (12, "AM"),
        1..=11 => (h24, "AM"),
        12 => (12, "PM"),
        _ => (h24 - 12, "PM"),
    };
    Some(format!("{} {h12}:{mi:02} {ampm}", &DAYS[day as usize][..3]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"<table class="net-schedule"><tr><th class="align-left">Name</th><th>Days</th></tr>
        <tr><td><strong><a href="/nets/view/silver-comet-amateur-radio-society">Silver Comet Amateur Radio Society Nite Out</a></strong></td>
        <td><div class="badges"><ul><li><div class="feature-box day" title="Monday">M</div></li><li><div class="feature-box day-off" title="Tuesday">T</div></li><li><div class="feature-box day-off" title="Wednesday">W</div></li><li><div class="feature-box day-off" title="Thursday">T</div></li><li><div class="feature-box day-off" title="Friday">F</div></li><li><div class="feature-box day-off" title="Saturday">S</div></li><li><div class="feature-box day-off" title="Sunday">S</div></li></ul></div></td>
        <td><div class="badges"><ul><li><a href="http://www.SilverCometARS.org" target="_blank"><div class="feature-box website" title="http://www.SilverCometARS.org">Website</div></a></li><li><div class="feature-box repeater" title="This net takes place on a repeater">Repeater</div></li><li><div class="feature-box echolink" title="EchoLink">EchoLink</div></li></ul></div></td>
        <td><div class="badges"><ul><li><div class="feature-box all-nets meters-2">2 Meters</div></li></ul></div></td></tr>
        <tr><td><strong><a href="/nets/view/tag-multi-mode-digital-net">TAG Multi Mode Digital Net</a></strong></td>
        <td><div class="badges"><ul><li><div class="feature-box day-off" title="Monday">M</div></li><li><div class="feature-box day" title="Tuesday">T</div></li><li><div class="feature-box day-off" title="Wednesday">W</div></li><li><div class="feature-box day-off" title="Thursday">T</div></li><li><div class="feature-box day-off" title="Friday">F</div></li><li><div class="feature-box day-off" title="Saturday">S</div></li><li><div class="feature-box day" title="Sunday">S</div></li></ul></div></td>
        <td><div class="badges"><ul><li><div class="feature-box dmr">DMR</div></li><li><div class="feature-box echolink">EchoLink</div></li><li><div class="feature-box internet">Internet</div></li></ul></div></td>
        <td><div class="badges"><ul></ul></div></td></tr></table>
        <a href="/nets/modes/echolink?page=2">2</a><a href="/nets/modes/echolink?page=9">9</a>"#;

    const DETAIL: &str = r#"<h1>GARA Sunday Evening Net&nbsp;<span class="edit"><a href="/nets/edit/x">Edit</a></span></h1>
        <table class="net-schedule"><tr><th>Local Time</th><th>UTC Time</th><th>Frequency</th><th>Net Control</th></tr>
        <tr><td>8:30  PM Sunday <span class="timezone">America/New_York</span></td><td>
          01:30 Monday          </td><td class="align-right">
              W4GSO Repeater <strong>145.150</strong> <span class="mhz">MHz</span><br>
          </td><td class="align-right"><a href="/nets/edit/x">Add an operator</a></td></tr></table>
        <h2>Net Details</h2><p><strong>Organizer</strong><br>Gara</p><p><strong>EchoLink Node</strong><br>W4GSO-R</p><p><strong>Province / State</strong><br>North Carolina</p><p><strong>Band</strong><br>2 Meters</p>"#;

    #[test]
    fn reads_the_list() {
        let (nets, last) = parse_list(LIST);
        assert_eq!(last, 9);
        assert_eq!(nets.len(), 2, "the header row isn't a net");
        let n = &nets[0];
        assert_eq!((n.slug.as_str(), n.name.as_str(), n.band.as_str()), ("silver-comet-amateur-radio-society", "Silver Comet Amateur Radio Society Nite Out", "2 Meters"));
        assert_eq!(n.days, [true, false, false, false, false, false, false]);
        assert_eq!(n.tags, ["Repeater", "EchoLink"]);
        assert_eq!(n.website.as_deref(), Some("http://www.SilverCometARS.org"));
        let t = &nets[1];
        assert_eq!(t.days, [false, true, false, false, false, false, true]);
        assert_eq!(t.tags, ["DMR", "EchoLink", "Internet"]);
        assert!(t.band.is_empty() && t.website.is_none());
    }

    #[test]
    fn reads_a_net_page() {
        let d = parse_detail(DETAIL);
        assert_eq!(d.name, "GARA Sunday Evening Net");
        assert_eq!(d.slots.len(), 1);
        let s = &d.slots[0];
        assert_eq!((s.local.as_str(), s.timezone.as_str(), s.utc.as_str()), ("8:30 PM Sunday", "America/New_York", "01:30 Monday"));
        assert_eq!(s.frequency, "W4GSO Repeater 145.150 MHz");
        assert_eq!(d.echolink_node.as_deref(), Some("W4GSO-R"));
        assert!(d.fields.contains(&("Organizer".into(), "Gara".into())) && d.fields.contains(&("Province / State".into(), "North Carolina".into())));
    }

    #[test]
    fn a_changed_page_yields_nothing_rather_than_junk() {
        assert!(parse_list("<html><body>Maintenance</body></html>").0.is_empty());
        let d = parse_detail("<html></html>");
        assert!(d.name.is_empty() && d.slots.is_empty() && d.echolink_node.is_none());
    }

    #[test]
    fn times_move_into_your_zone() {
        assert_eq!(in_my_time("01:30 Monday", -300).as_deref(), Some("Sun 8:30 PM"));
        assert_eq!(in_my_time("01:30 Monday", 0).as_deref(), Some("Mon 1:30 AM"));
        assert_eq!(in_my_time("23:00 Sunday", 120).as_deref(), Some("Mon 1:00 AM"), "wraps past the end of the week");
        assert_eq!(in_my_time("12:05 Wednesday", 0).as_deref(), Some("Wed 12:05 PM"));
        assert_eq!(in_my_time("00:15 Monday", -30).as_deref(), Some("Sun 11:45 PM"));
        assert_eq!(in_my_time("noon", 0), None);
    }

    /// Run against saved copies of the real pages:
    /// NETS_LIST=list.html NETS_DETAIL=detail.html cargo test real_pages -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_pages() {
        if let Ok(path) = std::env::var("NETS_LIST") {
            let (nets, last) = parse_list(&std::fs::read_to_string(path).unwrap());
            println!("{} nets on this page, {last} pages in all", nets.len());
            for n in nets.iter().take(4) {
                println!("  {:<55} days {} band {:<10} tags {:?} site {:?}", n.name, n.days.iter().map(|d| if *d { 'X' } else { '.' }).collect::<String>(), n.band, n.tags, n.website);
            }
            assert!(nets.len() >= 10 && nets.iter().all(|n| !n.name.is_empty() && !n.slug.is_empty()));
            assert!(nets.iter().any(|n| n.days.iter().any(|d| *d)), "some net should have a day");
            assert!(nets.iter().all(|n| n.tags.iter().any(|t| t == "EchoLink")), "every net on the EchoLink list is tagged EchoLink");
        }
        if let Ok(path) = std::env::var("NETS_DETAIL") {
            let d = parse_detail(&std::fs::read_to_string(path).unwrap());
            println!("{d:#?}");
            assert!(!d.name.is_empty() && !d.slots.is_empty());
        }
    }

    #[test]
    fn weekdays() {
        assert_eq!(weekday(0), 3); // 1970-01-01 was a Thursday
        assert_eq!(weekday(1_700_000_000), 1); // 2023-11-14 was a Tuesday
        assert_eq!(weekday(-86400), 2);
    }

    #[test]
    fn page_urls_are_checked() {
        assert_eq!(page_url("gara-sunday-evening-net").as_deref(), Some("https://netfinder.radio/nets/view/gara-sunday-evening-net"));
        assert_eq!(page_url("x/../../evil"), None);
        assert_eq!(page_url("a b"), None);
    }
}
