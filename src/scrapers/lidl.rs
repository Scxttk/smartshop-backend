use anyhow::{Context, Result, bail};
use std::collections::HashSet;

use crate::models::{Market, Offer};
use crate::scrapers::{store_finder, util};

// Lidl-Wochenangebote über die öffentliche marktguru-Web-API.
//
//   GET https://api.marktguru.de/api/v1/offers/search
//       ?as=web&limit=500&offset=...&q=lidl&zipCode=...
//       Header: x-apikey / x-clientkey (stehen im HTML von marktguru.de)
//
// Hintergrund: Die früher genutzte Onlineshop-Suche von lidl.de
// (/q/api/search?store=1, "In der Filiale"-Facet) enthält NUR die
// Non-Food-Aktionswaren (PARKSIDE, ESMARA, ...) und die undatierte Weinwelt —
// die wöchentlichen Lebensmittel-Filialangebote (Fleisch, Obst, Molkerei,
// "Super Samstag") sind dort nicht gelistet, weil sie nicht online verkauft
// werden. Die lidl.de-Kampagnenseiten (/c/billiger-montag/...) leiten auf die
// Online-Prospekte um; der Prospekt-Viewer (endpoints.leaflets.schwarz
// /v4/flyer) liefert nur Seitenbilder ohne strukturierte Produkte, und die
// Lidl-Plus-API ist OAuth-pflichtig. marktguru indexiert den kompletten
// Wochenprospekt (aktuelle + nächste Woche) strukturiert mit Preisen,
// Gültigkeit und Kategorien — inklusive der Lidl-Plus-Aktionen.
//
// Lidl-Angebote gelten praktisch bundesweit; die PLZ geht trotzdem mit, damit
// regionale Abweichungen (falls marktguru welche ausliefert) korrekt landen.

const API_URL: &str = "https://api.marktguru.de/api/v1/offers/search";
const KEYS_URL: &str = "https://www.marktguru.de/";
// Eingefrorene Web-Client-Keys als Fallback, falls das Auslesen aus dem
// marktguru-HTML scheitert (Stand 2026-07-18).
const FALLBACK_API_KEY: &str = "8Kk+pmbf7TgJ9nVj2cXeA7P5zBGv8iuutVVMRfOfvNE=";
const FALLBACK_CLIENT_KEY: &str = "WU/RH+PMGDi+gkZer3WbMelt6zcYHSTytNB7VpTia90=";
const PAGE_SIZE: usize = 500;
const MAX_OFFERS: usize = 2000;

/// Echte Filiale über den Store-Finder; None, wenn es im Umkreis der PLZ
/// keine Lidl-Filiale gibt. Die Filiale liefert Präsenz + Metadaten
/// (Name, ID, Koordinaten); die Angebote kommen von marktguru.
pub fn find_market(zip: &str) -> Result<Option<Market>> {
    Ok(store_finder::resolve("Lidl", store_finder::lidl_branch(zip), national()))
}

fn national() -> Market {
    Market::new("LIDL_DE", "Lidl Deutschland")
}

pub fn fetch_offers(market: &Market, zip: &str) -> Result<Vec<Offer>> {
    block_on(async {
        let client = util::async_client()?;
        let (api_key, client_key) = fetch_keys(&client).await;

        let mut offers = Vec::new();
        let mut seen = HashSet::new();
        let mut offset = 0usize;

        loop {
            let url = format!(
                "{API_URL}?as=web&limit={PAGE_SIZE}&offset={offset}&q=lidl&zipCode={zip}"
            );

            util::polite_pause(&url);
            let resp = client
                .get(&url)
                .header("x-apikey", &api_key)
                .header("x-clientkey", &client_key)
                .header("Accept", "application/json")
                .send()
                .await
                .with_context(|| util::ctx("Lidl", "Angebote laden", &url))?;

            if !resp.status().is_success() {
                bail!("[Lidl] Angebote laden lieferte HTTP {}: {url}", resp.status());
            }

            let raw: serde_json::Value = resp
                .json()
                .await
                .with_context(|| util::ctx("Lidl", "Angebote JSON parsen", &url))?;

            let total = raw.get("totalResults").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let results = raw
                .get("results")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            if results.is_empty() {
                break;
            }

            for item in &results {
                if let Some(offer) = parse_offer(item, &market.id) {
                    if seen.insert(offer.id.clone()) {
                        offers.push(offer);
                    }
                }
            }

            offset += results.len();
            if offset >= total.min(MAX_OFFERS) {
                break;
            }
        }

        Ok(offers)
    })
}

// Die Web-Keys stehen als "apiKey":"..." / "clientKey":"..." im HTML der
// marktguru-Startseite. Fehlschlag ist kein Abbruchgrund — dann greifen die
// eingefrorenen Fallback-Keys.
async fn fetch_keys(client: &reqwest::Client) -> (String, String) {
    let html = match client.get(KEYS_URL).send().await {
        Ok(resp) => resp.text().await.unwrap_or_default(),
        Err(_) => String::new(),
    };
    let extract = |key: &str| -> Option<String> {
        let marker = format!("\"{key}\":\"");
        let start = html.find(&marker)? + marker.len();
        let end = html[start..].find('"')?;
        Some(html[start..start + end].to_string())
    };
    match (extract("apiKey"), extract("clientKey")) {
        (Some(a), Some(c)) => (a, c),
        _ => (FALLBACK_API_KEY.to_string(), FALLBACK_CLIENT_KEY.to_string()),
    }
}

// Ein marktguru-Suchtreffer defensiv in ein Offer übersetzen; bei fehlenden
// Pflichtfeldern oder fremdem Händler None (Treffer wird übersprungen).
pub fn parse_offer(item: &serde_json::Value, market_id: &str) -> Option<Offer> {
    // q=lidl ist Volltextsuche — nur Treffer behalten, deren Händler Lidl ist.
    let is_lidl = item
        .get("advertisers")
        .and_then(|v| v.as_array())
        .is_some_and(|advs| {
            advs.iter().any(|a| {
                a.get("uniqueName")
                    .and_then(|v| v.as_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case("lidl"))
            })
        });
    if !is_lidl {
        return None;
    }

    let title = item
        .pointer("/product/name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();

    let price = item.get("price").and_then(|v| v.as_f64());
    let regular_price = item.get("oldPrice").and_then(|v| v.as_f64());

    // Verpackungsgröße aus volume/quantity/unit, z. B. "0.75 l" oder "3 x 1 Stk"
    let subtitle = item
        .pointer("/unit/shortName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .and_then(|unit| {
            let volume = item.get("volume").and_then(|v| v.as_f64()).filter(|v| *v > 0.0)?;
            let quantity = item.get("quantity").and_then(|v| v.as_f64()).unwrap_or(1.0);
            let base = format!("{} {unit}", fmt_num(volume));
            Some(if quantity > 1.0 { format!("{} x {base}", fmt_num(quantity)) } else { base })
        });

    // marktguru markiert markenlose Produkte mit dem Dummy "thisisnobrand123"
    let overline = item
        .pointer("/brand/name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("thisisnobrand123"))
        .map(String::from);

    let category = item
        .get("categories")
        .and_then(|v| v.as_array())
        .and_then(|cats| cats.first())
        .and_then(|c| c.get("name"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);

    // validityDates[0].from/to sind UTC-Zeitstempel ("2026-07-12T22:00:00Z"
    // = Montag 00:00 Europe/Berlin) — für das Datum nach Berlin umrechnen.
    let validity = item.pointer("/validityDates/0");
    let valid_from = validity
        .and_then(|v| v.get("from"))
        .and_then(|v| v.as_str())
        .and_then(utc_to_berlin_date);
    let valid_until = validity
        .and_then(|v| v.get("to"))
        .and_then(|v| v.as_str())
        .and_then(utc_to_berlin_date);

    // Bilder liegen auf dem marktguru-CDN unter einer festen URL pro Offer-ID.
    let images = match (
        item.get("id").and_then(|v| v.as_i64()),
        item.pointer("/images/count").and_then(|v| v.as_i64()).unwrap_or(0) > 0,
    ) {
        (Some(id), true) => {
            vec![format!("https://mg2de.b-cdn.net/api/v1/offers/{id}/images/default/0/medium.jpg")]
        }
        _ => Vec::new(),
    };

    let id = Offer::build_id(market_id, &title, valid_from.as_deref());

    Some(Offer {
        id,
        market_id: market_id.to_string(),
        title,
        subtitle,
        overline,
        price,
        regular_price,
        category,
        nutri_score: None,
        valid_from,
        valid_until,
        images,
        biozid: false,
        flyer_page: None,
    })
}

// "2026-07-12T22:00:00Z" -> "2026-07-13" (Europe/Berlin)
fn utc_to_berlin_date(ts: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.with_timezone(&chrono_tz::Europe::Berlin).format("%Y-%m-%d").to_string())
}

// 3.0 -> "3", 0.75 -> "0.75"
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v}") }
}

// main() ist synchron — eigener Runtime wie in penny.rs.
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
    #[ignore = "Live-Test gegen api.marktguru.de: cargo test lidl -- --ignored --nocapture"]
    fn live_fetch_offers() {
        let market = find_market("01219").expect("Markt").expect("Filiale");
        println!("Markt: {} ({})", market.name, market.id);

        let offers = fetch_offers(&market, "01219").expect("Angebote");
        println!("{} Angebote", offers.len());
        for o in offers.iter().take(5) {
            println!(
                "- {} | {:?} | {:?} € (statt {:?}) | {:?} | {:?} bis {:?}",
                o.title, o.subtitle, o.price, o.regular_price, o.category, o.valid_from, o.valid_until
            );
        }
        assert!(offers.len() >= 300, "Erwartet mehrere hundert Angebote, war {}", offers.len());

        let dated = offers.iter().filter(|o| o.valid_from.is_some()).count();
        println!("{dated}/{} Angebote mit valid_from", offers.len());
        assert!(dated >= 300, "Erwartet mehrere hundert datierte Angebote, war {dated}");

        // Kern der Umstellung: Der Mix muss Lebensmittel enthalten, nicht nur
        // Onlineshop-Non-Food. Mindestens 3 Lebensmittel-Zielkategorien nach
        // Enrichment (Titel + Roh-Kategorie), analog zum Penny-Mix.
        let mut food_cats = std::collections::HashSet::new();
        for o in &offers {
            let e = crate::enrich::enrich(&o.title, o.subtitle.as_deref(), o.category.as_deref());
            if matches!(
                e.category,
                "Obst & Gemüse" | "Fleisch & Wurst" | "Molkerei & Eier" | "Backwaren" | "Fisch"
            ) {
                food_cats.insert(e.category);
            }
        }
        println!("Lebensmittel-Kategorien: {food_cats:?}");
        assert!(
            food_cats.len() >= 3,
            "Erwartet >= 3 Lebensmittel-Kategorien, war {food_cats:?}"
        );
    }
}
