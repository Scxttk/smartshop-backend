use anyhow::{Context, Result, bail};

// Gemeinsamer HTTP-Helfer für Akamai-geschützte Seiten (Netto, ALDI Süd).
//
// Der Akamai-Bot-Schutz fingerprintet den TLS-Stack — reqwest/rustls wird
// konsequent mit HTTP 403 geblockt, während curl mit vollem
// Browser-Header-Satz durchkommt (verifiziert 2026-07). Deshalb laufen
// diese Requests über das System-curl (Präzedenzfall: rewe.rs shellt zum
// rewerse-CLI aus), mit Retries gegen sporadische 403.

pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const RETRIES: u32 = 3;

// GET über System-curl; liefert den Response-Body als String (nur bei HTTP 200).
pub fn curl_get(url: &str, extra_headers: &[(&str, &str)]) -> Result<String> {
    for attempt in 0..RETRIES {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
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
