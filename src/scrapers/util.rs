use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// Gemeinsamer HTTP-Helfer für Akamai-geschützte Seiten (Netto, ALDI Süd).
//
// Der Akamai-Bot-Schutz fingerprintet den TLS-Stack — reqwest/rustls wird
// konsequent mit HTTP 403 geblockt, während curl mit vollem
// Browser-Header-Satz durchkommt (verifiziert 2026-07). Deshalb laufen
// diese Requests über das System-curl (Präzedenzfall: rewe.rs shellt zum
// rewerse-CLI aus), mit Retries gegen sporadische 403.

pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const RETRIES: u32 = 3;

// Fehlerkontext einheitlich über alle Scraper: Kette, Schritt, URL.
pub fn ctx(chain: &str, step: &str, url: &str) -> String {
    format!("[{chain}] {step} fehlgeschlagen: {url}")
}

// reqwest-Clients mit dem gemeinsamen Browser-User-Agent.
pub fn async_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .context("HTTP-Client konnte nicht erstellt werden")
}

pub fn blocking_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .context("HTTP-Client konnte nicht erstellt werden")
}

// Höfliches Rate-Limiting: vor aufeinanderfolgenden Requests an denselben
// Host eine kleine, zufällig gestreute Pause (300-800 ms) einlegen.
// Erster Request an einen Host läuft ohne Verzögerung.
static LAST_REQUEST: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

pub fn polite_pause(url: &str) {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string();

    let wait = {
        let guard = LAST_REQUEST.lock().unwrap();
        guard.as_ref().and_then(|map| map.get(&host)).and_then(|last| {
            let delay = Duration::from_millis(300 + jitter_ms(500));
            delay.checked_sub(last.elapsed())
        })
    };
    if let Some(d) = wait {
        std::thread::sleep(d);
    }
    let mut guard = LAST_REQUEST.lock().unwrap();
    guard.get_or_insert_with(HashMap::new).insert(host, Instant::now());
}

// Pseudozufall aus der Systemuhr — reicht für Jitter, spart die rand-Dependency.
fn jitter_ms(range: u64) -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 % range)
        .unwrap_or(0)
}

// GET über System-curl; liefert den Response-Body als String (nur bei HTTP 200).
pub fn curl_get(url: &str, extra_headers: &[(&str, &str)]) -> Result<String> {
    for attempt in 0..RETRIES {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
        polite_pause(url);
        let mut cmd = std::process::Command::new("curl");
        cmd.arg("-s")
            .arg("-L")
            .arg("--compressed")
            .arg("--max-time")
            .arg("30")
            .arg("-w")
            .arg("\n%{http_code}")
            .args(["-H", &format!("User-Agent: {USER_AGENT}")])
            .args(["-H", "Accept-Language: de-DE,de;q=0.9,en;q=0.8"])
            .args(["-H", "Accept-Encoding: gzip, deflate, br"]);
        for (k, v) in extra_headers {
            cmd.args(["-H", &format!("{k}: {v}")]);
        }
        let output = cmd
            .arg(url)
            .output()
            .context("curl nicht gefunden — wird für diesen Scraper benötigt")?;

        let body = String::from_utf8_lossy(&output.stdout);
        let (content, status) = match body.rsplit_once('\n') {
            Some((c, s)) => (c, s.trim()),
            None => continue,
        };
        if status == "200" {
            return Ok(content.to_string());
        }
    }
    bail!("Wiederholte Fehler für {url} (Akamai-Blockade?)")
}

// GET ohne Redirect-Folgen; liefert das Redirect-Ziel (Location) einer 3xx-Antwort.
pub fn curl_redirect_url(url: &str, extra_headers: &[(&str, &str)]) -> Result<String> {
    for attempt in 0..RETRIES {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
        polite_pause(url);
        let mut cmd = std::process::Command::new("curl");
        cmd.arg("-s")
            .arg("-o")
            .arg(if cfg!(windows) { "NUL" } else { "/dev/null" })
            .arg("--max-time")
            .arg("30")
            .arg("-w")
            .arg("%{redirect_url}")
            .args(["-H", &format!("User-Agent: {USER_AGENT}")])
            .args(["-H", "Accept-Language: de-DE,de;q=0.9,en;q=0.8"])
            .args(["-H", "Accept-Encoding: gzip, deflate, br"]);
        for (k, v) in extra_headers {
            cmd.args(["-H", &format!("{k}: {v}")]);
        }
        let output = cmd
            .arg(url)
            .output()
            .context("curl nicht gefunden — wird für diesen Scraper benötigt")?;

        let target = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !target.is_empty() {
            return Ok(target);
        }
    }
    bail!("Kein Redirect-Ziel für {url} erhalten")
}
