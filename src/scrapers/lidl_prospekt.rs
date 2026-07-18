use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::Deserialize;

use crate::models::{Market, Offer};
use crate::scrapers::{store_finder, util};

// Lidl-Filialangebote aus dem Online-Prospekt via Schwarz/Leaflets-API +
// GitHub-Models-Vision-LLM.
//
// Hintergrund: Der marktguru-basierte Scraper (lidl.rs) bleibt der Default;
// dieses Modul ist eine zweite, LLM-gestützte Quelle direkt aus dem echten
// Wochenprospekt. Pipeline:
//   1. Overview  GET https://www.lidl.com/flyer/esi-overview/overview
//                    ?client_locale=lidl/de-DE&mode=iframe  -> Prospekt-Slugs
//   2. Flyer     GET https://endpoints.leaflets.schwarz/v4/flyer
//                    ?flyer_identifier=<slug>  -> JSON (Seiten + Gültigkeit)
//   3. Vorfilter der Seiten (keyWords/altText): Non-Food/Werbeseiten raus.
//   4. Seitenbilder (image, 1200px JPEG, signierte imgproxy-URL) laden.
//   5. Bild -> GitHub Models (gpt-4.1-mini, Fallback gpt-4o-mini) Vision.
//   6. LLM liefert name/price/unit; valid_from/valid_to kommen AUS dem
//      Flyer-JSON (offerStartDate/offerEndDate) — nie vom LLM (halluziniert
//      sonst das Jahr).
//
// Region: Das Overview listet ~14 Regionsvarianten pro Woche (Slug-Suffix =
// Region-Hash). Die passende Variante wird über die Absatzregion (AR) der
// PLZ-Filiale bestimmt: store_finder::lidl_region_code liefert das AR-Feld aus
// dem Bing-Store-Finder, das 1:1 dem `regions[].code` im Flyer-JSON entspricht
// (jede AR liegt in genau einer Variante — verifiziert für 01219/München/
// Hamburg/Köln/Berlin/Frankfurt/Stuttgart, 2026-07-18). Fallback ohne
// Store-Finder-Treffer: erste Nicht-Platzhaltervariante der Woche.

const OVERVIEW_URL: &str =
    "https://www.lidl.com/flyer/esi-overview/overview?client_locale=lidl%2Fde-DE&mode=iframe";
const FLYER_URL: &str = "https://endpoints.leaflets.schwarz/v4/flyer?flyer_identifier=";
const MODELS_URL: &str = "https://models.github.ai/inference/chat/completions";
const PRIMARY_MODEL: &str = "gpt-4.1-mini";
const FALLBACK_MODEL: &str = "gpt-4o-mini";
const TOKEN_ENV: &str = "GITHUB_MODELS_TOKEN";

// Free-Tier-Rate-Limit: 15 Req/min. Zwischen Vision-Calls throtteln.
const CALL_PAUSE_SECS: u64 = 4;
// Obergrenze an Vision-Calls pro Lauf (Kosten/Rate-Limit-Schutz).
const DEFAULT_MAX_PAGES: usize = 12;

/// Echte Filiale über den Store-Finder; None, wenn es im Umkreis der PLZ keine
/// Lidl-Filiale gibt. Identisch zu lidl.rs — die Präsenzprüfung ist dieselbe.
pub fn find_market(zip: &str) -> Result<Option<Market>> {
    Ok(store_finder::resolve("Lidl", store_finder::lidl_branch(zip), national()))
}

fn national() -> Market {
    Market::new("LIDL_DE", "Lidl Deutschland")
}

/// Vollständige Prospekt-Pipeline für eine PLZ. `max_pages` begrenzt die Zahl
/// der Vision-Calls (= gefilterte Angebotsseiten).
pub fn fetch_offers(market: &Market, zip: &str, max_pages: usize) -> Result<Vec<Offer>> {
    let token = std::env::var(TOKEN_ENV)
        .with_context(|| format!("{TOKEN_ENV} nicht gesetzt (Token aus `gh auth token`)"))?;

    // Absatzregion (AR) der Filiale VOR dem async-Runtime bestimmen: der
    // Store-Finder nutzt reqwest::blocking (eigener Runtime) und darf nicht aus
    // einem async-Kontext laufen. Finder-Fehler sind kein Abbruch — der
    // Fallback in resolve_flyer greift.
    let region = match store_finder::lidl_region_code(zip) {
        Ok(ar) => ar,
        Err(e) => {
            eprintln!("[Lidl-Prospekt] Regions-Lookup fehlgeschlagen ({e:#}) — Fallback-Variante.");
            None
        }
    };

    block_on(async {
        let client = util::async_client()?;

        // 1./2. Passende Wochenvariante (Region der PLZ) + Flyer-JSON.
        let (slug, flyer) = resolve_flyer(&client, region.as_deref()).await?;
        eprintln!(
            "[Lidl-Prospekt] Variante {slug} (Region-Codes {:?})",
            flyer.region_codes()
        );

        // 3. Vorfilter.
        let pages = offer_pages(&flyer.pages, max_pages);
        if pages.is_empty() {
            bail!("[Lidl-Prospekt] Keine Angebotsseiten nach Vorfilter in Slug {slug}");
        }

        // 4./5./6. Bild laden -> Vision -> Offers mit injizierten Daten.
        let mut offers = Vec::new();
        for (idx, page) in &pages {
            let bytes = download_image(&client, &page.image).await?;
            let raws = match vision_extract(&client, &token, &bytes).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[Lidl-Prospekt] Seite {idx} Vision fehlgeschlagen: {e:#}");
                    continue;
                }
            };
            for raw in raws {
                if let Some(o) = build_offer(
                    &raw,
                    &market.id,
                    flyer.offer_start_date.as_deref(),
                    flyer.offer_end_date.as_deref(),
                    *idx as i64,
                ) {
                    offers.push(o);
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(CALL_PAUSE_SECS));
        }

        Ok(dedup(offers))
    })
}

// --------------------------------------------------------------- Slug/Overview

/// Wochenvariante zur PLZ auflösen: Overview -> Slugs der laufenden Woche ->
/// Absatzregion (AR) der Filiale -> passende Variante samt Flyer-JSON.
///
/// Die AR-Zuordnung (store_finder::lidl_region_code) trifft genau eine Variante
/// (deren `regions[].code` die AR enthält). Ist der Store-Finder nicht
/// erreichbar oder passt keine Variante, greift der Fallback aus
/// `pick_region_variant` (erste Nicht-Platzhaltervariante der Woche).
async fn resolve_flyer(client: &reqwest::Client, target: Option<&str>) -> Result<(String, Flyer)> {
    let html = fetch_overview(client).await?;
    let slugs = parse_overview_slugs(&html);
    let week = week_slugs(&slugs, today_berlin());
    if week.is_empty() {
        bail!("[Lidl-Prospekt] Kein passender Aktionsprospekt-Slug ({} im Overview)", slugs.len());
    }

    // Alle Wochenvarianten laden (kleine JSONs, kein Vision) und passende wählen.
    let mut loaded: Vec<(String, Flyer)> = Vec::with_capacity(week.len());
    for slug in &week {
        loaded.push((slug.clone(), fetch_flyer(client, slug).await?));
    }
    let index: Vec<(String, Vec<String>)> = loaded
        .iter()
        .map(|(s, f)| (s.clone(), f.region_codes().into_iter().map(String::from).collect()))
        .collect();
    let chosen = pick_region_variant(&index, target)
        .expect("Woche nicht leer")
        .to_string();
    if let Some(ar) = target
        && !index.iter().any(|(s, codes)| *s == chosen && codes.iter().any(|c| c == ar))
    {
        eprintln!("[Lidl-Prospekt] Keine Variante mit Region-Code {ar} — Fallback {chosen}.");
    }
    let flyer = loaded.into_iter().find(|(s, _)| *s == chosen).map(|(_, f)| f).expect("gewählt geladen");
    Ok((chosen, flyer))
}

async fn fetch_overview(client: &reqwest::Client) -> Result<String> {
    util::polite_pause(OVERVIEW_URL);
    let resp = client
        .get(OVERVIEW_URL)
        .send()
        .await
        .with_context(|| util::ctx("Lidl-Prospekt", "Overview laden", OVERVIEW_URL))?;
    if !resp.status().is_success() {
        bail!("[Lidl-Prospekt] Overview lieferte HTTP {}", resp.status());
    }
    Ok(resp.text().await.unwrap_or_default())
}

/// Alle Varianten-Slugs derselben Woche, die `pick_slug` wählen würde
/// (laufende bzw. nächste kommende). Grundlage der Regionsauswahl.
pub fn week_slugs(slugs: &[String], today: &str) -> Vec<String> {
    let Some(chosen) = pick_slug(slugs, today) else {
        return Vec::new();
    };
    match slug_range(&chosen) {
        Some(range) => slugs
            .iter()
            .filter(|s| slug_range(s).as_ref() == Some(&range))
            .cloned()
            .collect(),
        None => vec![chosen],
    }
}

/// Aus geladenen Wochenvarianten `(slug, region_codes)` die zur Absatzregion
/// `ar` passende wählen. Reihenfolge: exakter Region-Treffer > erste
/// Nicht-Platzhaltervariante (Codes != nur "0") > erste überhaupt. None nur
/// bei leerer Eingabe.
pub fn pick_region_variant<'a>(
    variants: &'a [(String, Vec<String>)],
    ar: Option<&str>,
) -> Option<&'a str> {
    if let Some(ar) = ar
        && let Some((slug, _)) = variants.iter().find(|(_, codes)| codes.iter().any(|c| c == ar))
    {
        return Some(slug);
    }
    variants
        .iter()
        .find(|(_, codes)| !codes.iter().all(|c| c == "0"))
        .or_else(|| variants.first())
        .map(|(slug, _)| slug.as_str())
}

/// Alle `aktionsprospekt-DD-MM-YYYY-DD-MM-YYYY-<region>`-Slugs aus dem
/// Overview-HTML, dedupliziert in Reihenfolge des ersten Auftretens.
pub fn parse_overview_slugs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let needle = "aktionsprospekt-";
    let mut rest = html;
    while let Some(pos) = rest.find(needle) {
        let tail = &rest[pos..];
        let end = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(tail.len());
        let slug = &tail[..end];
        // Vollständiges Datumsmuster: aktionsprospekt-DD-MM-YYYY-DD-MM-YYYY-xxxx
        if slug.matches('-').count() >= 7 && !out.contains(&slug.to_string()) {
            out.push(slug.to_string());
        }
        rest = &tail[end..];
    }
    out
}

/// Slug wählen, dessen Gültigkeit `today` (YYYY-MM-DD) enthält; sonst den
/// ersten der nächsten kommenden Woche; sonst den ersten überhaupt.
pub fn pick_slug(slugs: &[String], today: &str) -> Option<String> {
    let current = slugs.iter().find(|s| {
        matches!(slug_range(s), Some((from, to)) if from.as_str() <= today && today <= to.as_str())
    });
    if let Some(s) = current {
        return Some(s.clone());
    }
    // Nächster in der Zukunft startender Prospekt (kleinstes from > today).
    let upcoming = slugs
        .iter()
        .filter_map(|s| slug_range(s).map(|(from, _)| (from, s)))
        .filter(|(from, _)| from.as_str() > today)
        .min_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, s)| s.clone());
    upcoming.or_else(|| slugs.first().cloned())
}

// "aktionsprospekt-13-07-2026-18-07-2026-4ff4e5" -> ("2026-07-13","2026-07-18")
fn slug_range(slug: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = slug.split('-').collect();
    // ["aktionsprospekt", d,m,y, d,m,y, region...]
    if parts.len() < 7 {
        return None;
    }
    let iso = |d: &str, m: &str, y: &str| -> Option<String> {
        if d.len() == 2 && m.len() == 2 && y.len() == 4 && [d, m, y].iter().all(|s| s.bytes().all(|b| b.is_ascii_digit())) {
            Some(format!("{y}-{m}-{d}"))
        } else {
            None
        }
    };
    let from = iso(parts[1], parts[2], parts[3])?;
    let to = iso(parts[4], parts[5], parts[6])?;
    Some((from, to))
}

// --------------------------------------------------------------- Flyer-JSON

#[derive(Debug, Clone, Deserialize)]
pub struct Flyer {
    #[serde(default, rename = "offerStartDate")]
    pub offer_start_date: Option<String>,
    #[serde(default, rename = "offerEndDate")]
    pub offer_end_date: Option<String>,
    #[serde(default)]
    pub regions: Vec<FlyerRegion>,
    #[serde(default)]
    pub pages: Vec<Page>,
}

/// Regionsschlüssel einer Prospektvariante. Der `code` entspricht dem AR-Feld
/// des Lidl-Store-Finders (store_finder::lidl_region_code); "0" markiert die
/// nationale Platzhaltervariante ohne regionale Angebote.
#[derive(Debug, Clone, Deserialize)]
pub struct FlyerRegion {
    #[serde(default)]
    pub code: String,
}

impl Flyer {
    /// Region-Codes dieser Variante.
    pub fn region_codes(&self) -> Vec<&str> {
        self.regions.iter().map(|r| r.code.as_str()).collect()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Page {
    #[serde(default)]
    pub image: String,
    #[serde(default, rename = "keyWords")]
    pub key_words: String,
    #[serde(default, rename = "altText")]
    pub alt_text: String,
}

/// Flyer-JSON parsen. Die Nutzdaten liegen unter dem Top-Level-Key `flyer`.
pub fn parse_flyer(raw: &serde_json::Value) -> Result<Flyer> {
    let node = raw.get("flyer").unwrap_or(raw);
    serde_json::from_value(node.clone()).context("Flyer-JSON-Struktur unerwartet")
}

async fn fetch_flyer(client: &reqwest::Client, slug: &str) -> Result<Flyer> {
    let url = format!("{FLYER_URL}{slug}");
    util::polite_pause(&url);
    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| util::ctx("Lidl-Prospekt", "Flyer laden", &url))?;
    if !resp.status().is_success() {
        bail!("[Lidl-Prospekt] Flyer lieferte HTTP {}: {url}", resp.status());
    }
    let raw: serde_json::Value = resp
        .json()
        .await
        .with_context(|| util::ctx("Lidl-Prospekt", "Flyer JSON parsen", &url))?;
    let flyer = parse_flyer(&raw)?;
    if flyer.pages.is_empty() {
        bail!("[Lidl-Prospekt] Flyer ohne Seiten (Struktur geändert?): {url}");
    }
    Ok(flyer)
}

// --------------------------------------------------------------- Vorfilter

// Marker reiner Werbe-/Image-Seiten und reiner Online-/Nicht-Filial-Kanäle.
// Bewusst NUR Seitentyp-/Kanal-Marker — keine Produktkategorien, weil echte
// Angebotsseiten oft nebenbei Non-Food nennen (z. B. "saisonale Pflanzen").
const EXCLUDE_MARKERS: &[&str] = &[
    "Werbeseite",
    "Image-Anzeige",
    "Titelseite",
    "Onlineshop",
    "Online-Angebot",
    "Karriere",
    "Fotoprodukte",
    "Lidl Reisen",
    "Reiseangebot",
    "Promotion",
    "Mobilfunk",
    "Lidl Connect",
];

// Positive Marker: Angebotsseiten mit Lebensmitteln/Getränken. Die Seite muss
// mindestens einen davon nennen, damit reine Non-Food-Seiten (Parkside,
// Esmara, Livarno …) ohne Lebensmittelbezug herausfallen.
const INCLUDE_MARKERS: &[&str] = &[
    "Angebot", "Milch", "Wurst", "Käse", "Fleisch", "Fisch", "Obst", "Gemüse", "Backshop",
    "Backwaren", "Süßwaren", "Schokolade", "Kaffee", "Getränke", "Molkerei", "Frische",
    "Knabber", "Nuss", "Wein", "Wochenend", "Rabatt",
];

/// Angebotsseite? Ausgeschlossen bei Werbe-/Online-Kanal-Markern, eingeschlossen
/// nur bei einem Lebensmittel-/Angebots-Marker im altText.
pub fn is_offer_page(page: &Page) -> bool {
    let alt = &page.alt_text;
    if EXCLUDE_MARKERS.iter().any(|m| alt.contains(m)) {
        return false;
    }
    INCLUDE_MARKERS.iter().any(|m| alt.contains(m)) && !page.image.is_empty()
}

/// Gefilterte Angebotsseiten in Lesereihenfolge (0-basierter Seitenindex),
/// höchstens `max_pages`.
pub fn offer_pages(pages: &[Page], max_pages: usize) -> Vec<(usize, &Page)> {
    pages
        .iter()
        .enumerate()
        .filter(|(_, p)| is_offer_page(p))
        .take(max_pages.min(DEFAULT_MAX_PAGES))
        .collect()
}

// --------------------------------------------------------------- Bild + Vision

async fn download_image(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    util::polite_pause(url);
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| util::ctx("Lidl-Prospekt", "Seitenbild laden", url))?;
    if !resp.status().is_success() {
        bail!("[Lidl-Prospekt] Seitenbild lieferte HTTP {}: {url}", resp.status());
    }
    Ok(resp.bytes().await.context("Seitenbild-Bytes lesen")?.to_vec())
}

const VISION_PROMPT: &str = "Du bist ein Extraktor für deutsche Supermarkt-Prospekte. \
Extrahiere ALLE Lebensmittel- und Getränke-Angebote mit sichtbarem Preis von dieser \
Lidl-Prospektseite. Gib AUSSCHLIESSLICH ein JSON-Array zurück, jedes Element mit den \
Schlüsseln: \"name\" (Produktname als String), \"price\" (Angebotspreis in Euro als Zahl, \
z.B. 1.99), \"unit\" (Menge/Einheit als String, optional, z.B. \"500 g\" oder \"1 kg\"). \
Keine Datumsangaben. Keine Non-Food-Artikel, keine reinen Werbetexte ohne Preis. \
Wenn kein Preis erkennbar ist, lass das Angebot weg. Nur das JSON-Array, kein weiterer Text.";

#[derive(Debug, Clone, Deserialize)]
pub struct RawOffer {
    pub name: String,
    // Vision-Modelle liefern den Preis mal als Zahl (1.99), mal als String
    // ("1,99", "1.99 €", "€ 2,49"). Ohne toleranten Deserializer würde ein
    // einziger String-Preis das Parsen des GESAMTEN Arrays scheitern lassen
    // und damit die ganze Seite verwerfen. Darum: Zahl ODER String akzeptieren,
    // Komma als Dezimaltrenner und Währungssymbole/Text robust abstreifen.
    #[serde(default, deserialize_with = "deserialize_price")]
    pub price: Option<f64>,
    #[serde(default)]
    pub unit: Option<String>,
}

fn deserialize_price<'de, D>(de: D) -> std::result::Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<serde_json::Value>::deserialize(de)? {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(n)) => Ok(n.as_f64()),
        Some(serde_json::Value::String(s)) => Ok(parse_price_str(&s)),
        // Unerwarteter Typ (bool/array/object): defensiv als None, nicht Fehler.
        Some(_) => Ok(None),
    }
}

/// "1,99 €" / "€ 2.49" / "ab 0,89" -> Some(1.99) usw. Nimmt die erste
/// Dezimalzahl, behandelt Komma als Dezimaltrenner. None, wenn keine Zahl.
pub fn parse_price_str(s: &str) -> Option<f64> {
    let mut num = String::new();
    for c in s.chars() {
        match c {
            '0'..='9' => num.push(c),
            '.' | ',' => num.push('.'),
            _ if num.is_empty() => {} // führenden Text ("ab", "€") überspringen
            _ => break,               // Zahl fertig, Rest (Einheit) ignorieren
        }
    }
    // Mehrere Trenner (z. B. "1.234,50") auf den letzten reduzieren.
    if num.matches('.').count() > 1 {
        if let Some(idx) = num.rfind('.') {
            num = format!("{}.{}", num[..idx].replace('.', ""), &num[idx + 1..]);
        }
    }
    num.parse::<f64>().ok().filter(|v| v.is_finite())
}

async fn vision_extract(
    client: &reqwest::Client,
    token: &str,
    image: &[u8],
) -> Result<Vec<RawOffer>> {
    let data_uri = format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(image)
    );
    // Erst Primär-, bei Fehler Fallback-Modell.
    let mut last_err = None;
    for model in [PRIMARY_MODEL, FALLBACK_MODEL] {
        match call_model(client, token, model, &data_uri).await {
            Ok(content) => return Ok(parse_llm_response(&content)),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap())
}

async fn call_model(
    client: &reqwest::Client,
    token: &str,
    model: &str,
    data_uri: &str,
) -> Result<String> {
    let body = serde_json::json!({
        "model": model,
        "temperature": 0.1,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": VISION_PROMPT},
                {"type": "image_url", "image_url": {"url": data_uri}}
            ]
        }]
    });
    let resp = client
        .post(MODELS_URL)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .with_context(|| util::ctx("Lidl-Prospekt", "Vision-Call", MODELS_URL))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("[Lidl-Prospekt] Vision-Call ({model}) HTTP {status}");
    }
    let parsed: serde_json::Value = serde_json::from_str(&text).context("Vision-Antwort JSON")?;
    parsed
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .context("Vision-Antwort ohne content")
}

/// LLM-Antwort tolerant parsen: Markdown-Code-Fences strippen, JSON-Array
/// extrahieren. Leeres Array bei Unparsbarem (keine Panik).
pub fn parse_llm_response(content: &str) -> Vec<RawOffer> {
    let cleaned = strip_fences(content);
    // Auf das erste JSON-Array eingrenzen (Modell hängt selten Text an).
    let slice = match (cleaned.find('['), cleaned.rfind(']')) {
        (Some(a), Some(b)) if b > a => &cleaned[a..=b],
        _ => return Vec::new(),
    };
    serde_json::from_str::<Vec<RawOffer>>(slice).unwrap_or_default()
}

fn strip_fences(s: &str) -> String {
    let t = s.trim();
    let t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t);
    t.trim().to_string()
}

// --------------------------------------------------------------- Offer-Bau

/// Ein RawOffer in ein Offer übersetzen; valid_from/until werden AUS dem
/// Flyer-JSON injiziert (nie vom LLM). None bei leerem Namen oder
/// unplausiblem Preis.
pub fn build_offer(
    raw: &RawOffer,
    market_id: &str,
    valid_from: Option<&str>,
    valid_until: Option<&str>,
    flyer_page: i64,
) -> Option<Offer> {
    let title = raw.name.trim();
    if title.is_empty() {
        return None;
    }
    // Preis-Plausibilität: 0,10–100 €.
    let price = raw.price.filter(|p| (0.10..=100.0).contains(p));
    price?;

    let subtitle = raw
        .unit
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let valid_from = valid_from.map(String::from);
    let id = Offer::build_id(market_id, title, valid_from.as_deref());

    Some(Offer {
        id,
        market_id: market_id.to_string(),
        title: title.to_string(),
        subtitle,
        overline: None,
        price,
        regular_price: None,
        category: None,
        nutri_score: None,
        valid_from,
        valid_until: valid_until.map(String::from),
        images: Vec::new(),
        biozid: false,
        flyer_page: Some(flyer_page),
    })
}

// Duplikate über die Offer-ID entfernen (gleicher Name kann mehrfach vorkommen).
fn dedup(offers: Vec<Offer>) -> Vec<Offer> {
    let mut seen = std::collections::HashSet::new();
    offers.into_iter().filter(|o| seen.insert(o.id.clone())).collect()
}

// Heutiges Datum in Europe/Berlin als "YYYY-MM-DD".
fn today_berlin() -> &'static str {
    // In einen leakenden String cachen, damit die Signatur &str bleibt.
    use std::sync::OnceLock;
    static TODAY: OnceLock<String> = OnceLock::new();
    TODAY.get_or_init(|| {
        chrono::Utc::now()
            .with_timezone(&chrono_tz::Europe::Berlin)
            .format("%Y-%m-%d")
            .to_string()
    })
}

fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(fut)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_price_str_handles_string_formats() {
        assert_eq!(parse_price_str("1,99"), Some(1.99));
        assert_eq!(parse_price_str("1.99"), Some(1.99));
        assert_eq!(parse_price_str("1,99 €"), Some(1.99));
        assert_eq!(parse_price_str("€ 2,49"), Some(2.49));
        assert_eq!(parse_price_str("ab 0,89"), Some(0.89));
        assert_eq!(parse_price_str("1.234,50"), Some(1234.50));
        assert_eq!(parse_price_str("keine Zahl"), None);
        assert_eq!(parse_price_str(""), None);
    }

    #[test]
    fn deserialize_price_accepts_number_or_string_without_dropping_array() {
        // Gemischtes Array (Zahl, deutscher String, Währungsstring, null):
        // ein einziger String-Preis darf nicht das ganze Array killen.
        let raws: Vec<RawOffer> = serde_json::from_str(
            r#"[
                {"name":"A","price":1.99},
                {"name":"B","price":"2,49 €"},
                {"name":"C","price":"€ 0,89"},
                {"name":"D","price":null}
            ]"#,
        )
        .expect("mixed price types parsen");
        assert_eq!(raws.len(), 4);
        assert_eq!(raws[0].price, Some(1.99));
        assert_eq!(raws[1].price, Some(2.49));
        assert_eq!(raws[2].price, Some(0.89));
        assert_eq!(raws[3].price, None);
    }

    #[test]
    fn parse_llm_response_survives_string_prices() {
        // Vollständiger Weg inkl. Fences: früher hätte ein String-Preis das
        // ganze Array (und damit die Seite) verworfen.
        let content = "```json\n[{\"name\":\"Butter\",\"price\":\"1,49 €\",\"unit\":\"250 g\"}]\n```";
        let raws = parse_llm_response(content);
        assert_eq!(raws.len(), 1);
        let offer = build_offer(&raws[0], "LIDL_DE", Some("2026-07-13"), Some("2026-07-18"), 3)
            .expect("Butter-Angebot");
        assert_eq!(offer.price, Some(1.49));
        assert_eq!(offer.subtitle.as_deref(), Some("250 g"));
    }

    #[test]
    #[ignore = "Live-Test gegen Schwarz-API + GitHub Models: \
        GITHUB_MODELS_TOKEN=$(gh auth token) cargo test lidl_prospekt_live -- --ignored --nocapture"]
    fn lidl_prospekt_live() {
        let market = find_market("01219").expect("Markt").expect("Filiale");
        println!("Markt: {} ({})", market.name, market.id);
        let offers = fetch_offers(&market, "01219", 5).expect("Angebote");
        println!("{} Angebote", offers.len());
        for o in &offers {
            println!(
                "- {} | {:?} | {:?} € | {:?}..{:?} | Seite {:?}",
                o.title, o.subtitle, o.price, o.valid_from, o.valid_until, o.flyer_page
            );
        }
        assert!(offers.len() >= 15, "Erwartet >= 15 Angebote, war {}", offers.len());
    }
}
