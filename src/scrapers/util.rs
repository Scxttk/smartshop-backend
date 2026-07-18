use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

// Hosts, deren Akamai-Session in diesem Prozess bereits aufgewärmt wurde.
static WARMED_HOSTS: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

// Host-Anteil einer URL ("https://www.netto-online.de/x" -> "www.netto-online.de").
fn host_of(url: &str) -> &str {
    url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or(url)
}

// Persistente Cookie-Jar-Datei pro Host im Temp-Verzeichnis. Akamai rotiert
// seine Session-Cookies (_abck/bm_sz/ak_bmsc) bei jeder Antwort — wer sie nicht
// mit -b/-c auf dieselbe Datei zurückschreibt, verliert die Session und wird
// beim nächsten Request mit 403 geblockt.
fn cookie_jar_path(host: &str) -> PathBuf {
    let safe: String =
        host.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    std::env::temp_dir().join(format!("smartshop_cookies_{safe}.txt"))
}

// Warm-up-GET, der die Akamai-Session-Cookies in die Jar schreibt (folgt
// Redirects). Fehler werden geschluckt — der eigentliche Download retryt ohnehin.
fn curl_warmup(url: &str, headers: &[(&str, &str)], jar: &Path) {
    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-s")
        .arg("-L")
        .arg("--compressed")
        .arg("--max-time")
        .arg("30")
        .arg("-o")
        .arg(if cfg!(windows) { "NUL" } else { "/dev/null" })
        .arg("-c")
        .arg(jar)
        .arg("-b")
        .arg(jar)
        .args(["-H", &format!("User-Agent: {USER_AGENT}")])
        .args(["-H", "Accept-Language: de-DE,de;q=0.9,en;q=0.8"])
        .args(["-H", "Accept-Encoding: gzip, deflate, br"]);
    for (k, v) in headers {
        cmd.args(["-H", &format!("{k}: {v}")]);
    }
    let _ = cmd.arg(url).output();
}

/// Binärdatei (Bild) über System-curl laden — für Akamai-geschützte CDNs (z. B.
/// netto-online.de), die reqwest/rustls konsequent mit 403 blocken. Nutzt eine
/// persistente Cookie-Jar pro Host; ist `warmup` gesetzt, wird die zugehörige
/// Seite besucht, um die Session-Cookies zu setzen (einmal vorab und vor jedem
/// Retry). Liefert (Bytes, Content-Type). Retries gegen sporadische 403 durch
/// Akamai-Rate-Limiting.
pub fn curl_get_bytes(
    url: &str,
    extra_headers: &[(&str, &str)],
    warmup: Option<(&str, &[(&str, &str)])>,
) -> Result<(Vec<u8>, String)> {
    let host = host_of(url).to_string();
    let jar = cookie_jar_path(&host);
    // Einmal pro Host pro Prozess aufwärmen — unabhängig von einer eventuell
    // veralteten Jar-Datei aus einem früheren Lauf (deren _abck kann tot sein).
    if let Some((wurl, wheaders)) = warmup {
        let first = {
            let mut guard = WARMED_HOSTS.lock().unwrap();
            guard.get_or_insert_with(std::collections::HashSet::new).insert(host.clone())
        };
        if first {
            curl_warmup(wurl, wheaders, &jar);
        }
    }

    let body_file = std::env::temp_dir()
        .join(format!("smartshop_dl_{}.bin", std::process::id()));

    for attempt in 0..RETRIES {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(3));
            // Session erneuern — bei 403 ist meist das _abck-Cookie „verbraucht".
            if let Some((wurl, wheaders)) = warmup {
                curl_warmup(wurl, wheaders, &jar);
            }
        }
        polite_pause(url);
        let mut cmd = std::process::Command::new("curl");
        cmd.arg("-s")
            .arg("--compressed")
            .arg("--max-time")
            .arg("30")
            .arg("-o")
            .arg(&body_file)
            .arg("-c")
            .arg(&jar)
            .arg("-b")
            .arg(&jar)
            .arg("-w")
            .arg("%{http_code}\t%{content_type}")
            .args(["-H", &format!("User-Agent: {USER_AGENT}")])
            .args(["-H", "Accept-Language: de-DE,de;q=0.9,en;q=0.8"])
            .args(["-H", "Accept-Encoding: gzip, deflate, br"]);
        for (k, v) in extra_headers {
            cmd.args(["-H", &format!("{k}: {v}")]);
        }
        let output = cmd
            .arg(url)
            .output()
            .context("curl nicht gefunden — wird für Akamai-geschützte Bilder benötigt")?;

        let meta = String::from_utf8_lossy(&output.stdout);
        let (status, ctype) = match meta.rsplit_once('\t') {
            Some((s, c)) => (s.trim(), c.trim()),
            None => continue,
        };
        if status == "200" {
            let bytes = std::fs::read(&body_file).context("Bild-Tempdatei lesen")?;
            let _ = std::fs::remove_file(&body_file);
            let ctype =
                if ctype.is_empty() { "image/jpeg".to_string() } else { ctype.to_string() };
            return Ok((bytes, ctype));
        }
    }
    let _ = std::fs::remove_file(&body_file);
    bail!("Bild-Download via curl fehlgeschlagen (Akamai-Blockade?): {url}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression für die Akamai-Bild-403: das reqwest-CDN von Netto blockt,
    // curl_get_bytes mit aufgewärmter Cookie-Session lädt das Bild als WebP.
    // Live: cargo test --lib util::tests::netto_image -- --ignored --nocapture
    #[test]
    #[ignore = "Live-Test gegen netto-online.de (Akamai)"]
    fn netto_image_downloads_via_curl_session() {
        let url = "https://www.netto-online.de/media_nfs/images/2026-29/\
                   42052-450x450-Kirschen-neu-n.webp";
        let warmup: (&str, &[(&str, &str)]) = (
            "https://www.netto-online.de/filialangebote/1",
            &[
                ("Cookie", "netto_user_stores_id=4816"),
                ("Sec-Fetch-Site", "none"),
                ("Sec-Fetch-Mode", "navigate"),
                ("Sec-Fetch-Dest", "document"),
            ],
        );
        let headers: [(&str, &str); 4] = [
            ("Accept", "image/avif,image/webp,*/*"),
            ("Referer", "https://www.netto-online.de/filialangebote/1"),
            ("Sec-Fetch-Mode", "no-cors"),
            ("Sec-Fetch-Dest", "image"),
        ];
        let (bytes, ctype) =
            curl_get_bytes(url, &headers, Some(warmup)).expect("Bild via curl-Session");
        println!("{} bytes, content-type={ctype}", bytes.len());
        assert!(bytes.len() > 5000, "zu klein, evtl. Fehlerseite: {} bytes", bytes.len());
        assert!(ctype.contains("image"), "kein Bild-Content-Type: {ctype}");
        // WebP-Signatur: RIFF....WEBP
        assert_eq!(&bytes[0..4], b"RIFF", "keine RIFF/WebP-Signatur");
        assert_eq!(&bytes[8..12], b"WEBP", "keine WEBP-Signatur");
    }
}
