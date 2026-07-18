//! Spiegelt Produktbilder in den öffentlichen Supabase-Storage-Bucket
//! `offer-images`. Die iOS-App bekommt dadurch stabile URLs statt der
//! rotierenden oder hotlink-geschützten Händler-CDNs.
//!
//! Idempotent & content-adressiert: Der Objektpfad leitet sich aus einem
//! sha256 der Quell-URL ab, der Upload nutzt `x-upsert`. Gleiches Bild ->
//! gleicher Pfad -> kein Duplikat. Den teuren Download sparen sich Aufrufer
//! über den lokalen SQLite-Cache (`db::cached_image_url`).

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::push::PushConfig;
use crate::scrapers::util;

pub const BUCKET: &str = "offer-images";

/// Bild-CDNs, die hinter Akamai liegen und reqwest/rustls mit 403 blocken.
/// Solche Bilder müssen — wie schon die HTML-Seiten der Ketten — über
/// System-curl mit einer aufgewärmten Cookie-Session geladen werden. Liefert
/// (Warm-up-URL, Warm-up-Header), sonst None (Standard-reqwest-Pfad).
///
/// Der Netto-Warm-up braucht eine gerenderte Filialangebote-Seite (HTTP 200);
/// eine beliebige gültige Filiale genügt, darum ist die Store-ID fix — sie
/// dient nur dazu, dass Akamai gültige Session-Cookies ausstellt.
fn akamai_image_warmup(url: &str) -> Option<(&'static str, &'static [(&'static str, &'static str)])> {
    if url.contains("netto-online.de") {
        Some((
            "https://www.netto-online.de/filialangebote/1",
            &[
                ("Cookie", "netto_user_stores_id=4816"),
                ("Sec-Fetch-Site", "none"),
                ("Sec-Fetch-Mode", "navigate"),
                ("Sec-Fetch-Dest", "document"),
                ("Sec-Fetch-User", "?1"),
            ],
        ))
    } else {
        None
    }
}

/// Deterministischer Objektpfad: sha256(Quell-URL) + Datei-Endung der Quelle.
pub fn object_path(source_url: &str) -> String {
    let hash = Sha256::digest(source_url.as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    format!("{hex}.{}", extension(source_url))
}

/// Datei-Endung aus der URL (ohne Query/Fragment), auf bekannte Bildformate
/// beschränkt; Fallback "jpg".
fn extension(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let ext = path
        .rsplit('/')
        .next()
        .and_then(|file| file.rsplit_once('.'))
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "avif" => ext,
        _ => "jpg".to_string(),
    }
}

/// Öffentliche Bucket-URL für einen Objektpfad.
pub fn public_url(base_url: &str, path: &str) -> String {
    format!("{base_url}/storage/v1/object/public/{BUCKET}/{path}")
}

/// Bild vom Händler laden und in den Bucket hochladen; liefert die öffentliche
/// Bucket-URL. Idempotent dank content-adressiertem Pfad + `x-upsert` — ein
/// erneuter Aufruf für dasselbe Bild überschreibt dasselbe Objekt.
pub fn mirror(
    client: &reqwest::blocking::Client,
    cfg: &PushConfig,
    source_url: &str,
) -> Result<String> {
    let path = object_path(source_url);

    // Bild vom Händler-CDN laden. Akamai-geschützte Hosts (Netto) blocken
    // reqwest mit 403 — die gehen über System-curl mit Cookie-Session.
    let (bytes, content_type) = match akamai_image_warmup(source_url) {
        Some((warmup_url, warmup_headers)) => {
            let img_headers: [(&str, &str); 5] = [
                ("Accept", "image/avif,image/webp,image/apng,*/*"),
                ("Referer", warmup_url),
                ("Sec-Fetch-Site", "same-origin"),
                ("Sec-Fetch-Mode", "no-cors"),
                ("Sec-Fetch-Dest", "image"),
            ];
            let (bytes, content_type) = util::curl_get_bytes(
                source_url,
                &img_headers,
                Some((warmup_url, warmup_headers)),
            )
            .with_context(|| format!("Bild-Download fehlgeschlagen: {source_url}"))?;
            (bytes, content_type)
        }
        None => {
            let resp = client
                .get(source_url)
                .send()
                .with_context(|| format!("Bild-Download fehlgeschlagen: {source_url}"))?;
            if !resp.status().is_success() {
                bail!("Bild-Download {source_url}: HTTP {}", resp.status());
            }
            let content_type = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("image/jpeg")
                .to_string();
            let bytes = resp
                .bytes()
                .context("Bild-Bytes lesen fehlgeschlagen")?
                .to_vec();
            (bytes, content_type)
        }
    };

    // In den Bucket hochladen (idempotent via x-upsert).
    let upload_url = format!("{}/storage/v1/object/{BUCKET}/{path}", cfg.base_url);
    let resp = client
        .post(&upload_url)
        .header("apikey", &cfg.api_key)
        .header("Authorization", format!("Bearer {}", cfg.api_key))
        .header("Content-Type", content_type)
        .header("x-upsert", "true")
        .body(bytes)
        .send()
        .with_context(|| format!("Bild-Upload fehlgeschlagen: {path}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        let excerpt: String = body.chars().take(200).collect();
        bail!("Bild-Upload {path}: HTTP {status}: {excerpt}");
    }

    Ok(public_url(&cfg.base_url, &path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_path_is_deterministic() {
        let url = "https://cdn.example/wassermelone.png";
        assert_eq!(object_path(url), object_path(url));
        // andere URL -> anderer Pfad
        assert_ne!(object_path(url), object_path("https://cdn.example/gouda.png"));
    }

    #[test]
    fn object_path_keeps_known_extension() {
        assert!(object_path("https://x/a.png").ends_with(".png"));
        assert!(object_path("https://x/a.jpeg").ends_with(".jpeg"));
        // Query wird ignoriert
        assert!(object_path("https://x/a.webp?w=450").ends_with(".webp"));
    }

    #[test]
    fn object_path_falls_back_to_jpg() {
        assert!(object_path("https://x/image").ends_with(".jpg"));
        assert!(object_path("https://x/a.bin").ends_with(".jpg"));
    }

    #[test]
    fn public_url_points_at_bucket() {
        assert_eq!(
            public_url("https://p.supabase.co", "abc.png"),
            "https://p.supabase.co/storage/v1/object/public/offer-images/abc.png"
        );
    }
}
