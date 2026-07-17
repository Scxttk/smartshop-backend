use anyhow::{Context, Result, bail};
use std::collections::HashSet;

use crate::models::{Market, Offer};

// Penny-Angebote über die öffentlichen JSON-Endpoints von penny.de.
// Kein Client-Zertifikat nötig, nur ein Browser-User-Agent.
//
// Märkte:     GET /.rest/market                     -> Array aller Märkte (PLZ-Filterung clientseitig)
// Kategorien: aus dem HTML von /angebote            -> data-category-name + data-current-week/-next-week
// Angebote:   GET /.rest/offers/by-category/{JAHR-WOCHE}/{kategorie}?region={sellingRegion}
//             -> { "offerTiles": [...] }

const BASE_URL: &str = "https://www.penny.de";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126 Safari/537.36";

pub fn find_market(zip: &str) -> Result<Market> {
    block_on(async {
        let client = build_client()?;
        let markets = fetch_markets(&client).await?;

        // Exakte PLZ bevorzugen, sonst den Markt mit der numerisch nächsten PLZ nehmen
        let target: i64 = zip.parse().with_context(|| format!("Ungültige PLZ: {zip}"))?;
        let market = markets
            .iter()
            .filter_map(|m| {
                let mzip: i64 = m.get("zipCode")?.as_str()?.parse().ok()?;
                Some(((mzip - target).abs(), m))
            })
            .min_by_key(|(dist, _)| *dist)
            .map(|(_, m)| m)
            .with_context(|| format!("Kein Penny-Markt für PLZ {zip} gefunden"))?;

        let id = market
            .get("wwIdent")
            .and_then(|v| v.as_str())
            .context("wwIdent fehlt in der Markt-Antwort")?;
        let name = market
            .get("marketName")
            .and_then(|v| v.as_str())
            .unwrap_or("PENNY");

        Ok(Market { id: id.to_string(), name: name.to_string() })
    })
}

pub fn fetch_offers(market: &Market) -> Result<Vec<Offer>> {
    block_on(async {
        let client = build_client()?;

        // sellingRegion des Markts über die Marktliste ermitteln
        let markets = fetch_markets(&client).await?;
        let region = markets
            .iter()
            .find(|m| m.get("wwIdent").and_then(|v| v.as_str()) == Some(market.id.as_str()))
            .and_then(|m| m.get("sellingRegion").and_then(|v| v.as_str()))
            .map(String::from);

        // Kategorien und Wochen stehen im HTML der Angebotsseite
        let html = client
            .get(format!("{BASE_URL}/angebote"))
            .send()
            .await
            .context("Penny-Angebotsseite-Request fehlgeschlagen")?
            .text()
            .await
            .context("Penny-Angebotsseite konnte nicht gelesen werden")?;

        let categories = extract_attr_values(&html, "data-category-name");
        if categories.is_empty() {
            bail!("Keine Angebots-Kategorien auf penny.de/angebote gefunden");
        }
        let current_week = extract_attr_values(&html, "data-current-week")
            .into_iter()
            .next()
            .context("data-current-week fehlt auf penny.de/angebote")?;
        let next_week = extract_attr_values(&html, "data-next-week").into_iter().next();

        let mut offers = Vec::new();
        let mut weeks = vec![current_week];
        weeks.extend(next_week);

        for week in &weeks {
            let (valid_from, valid_until) = week_dates(week)?;
            let mut seen = HashSet::new();

            for category in &categories {
                let mut url = format!("{BASE_URL}/.rest/offers/by-category/{week}/{category}");
                if let Some(r) = &region {
                    url.push_str(&format!("?region={r}"));
                }

                let resp = client
                    .get(&url)
                    .send()
                    .await
                    .with_context(|| format!("Penny-Angebote-Request fehlgeschlagen: {url}"))?;

                // Nicht jede Kategorie existiert in jeder Region/Woche
                if !resp.status().is_success() {
                    continue;
                }

                let raw: serde_json::Value = match resp.json().await {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let tiles = raw
                    .get("offerTiles")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                for tile in &tiles {
                    let Some(title) = tile.get("title").and_then(|v| v.as_str()) else { continue };
                    let title = title.to_string();

                    // Kategorien überschneiden sich (z. B. top-angebote vs. food)
                    if !seen.insert(title.to_lowercase()) {
                        continue;
                    }

                    let subtitle = tile.get("quantity").and_then(|v| v.as_str()).map(String::from);
                    let overline = tile
                        .get("headline")
                        .or_else(|| tile.get("actionMarker"))
                        .and_then(|v| v.as_str())
                        .map(String::from);

                    let price = tile.get("price").and_then(json_price);
                    let regular_price = tile
                        .get("listPrice")
                        .and_then(json_price)
                        .or_else(|| tile.get("crossOutPrice").and_then(json_price))
                        .or_else(|| tile.get("originalPrice").and_then(json_price));

                    let images: Vec<String> = tile
                        .get("imageRendition")
                        .and_then(|r| {
                            ["tileLg", "tileMd", "tileSm", "tileXs"]
                                .iter()
                                .find_map(|k| r.get(k).and_then(|v| v.as_str()))
                        })
                        .map(|s| vec![s.to_string()])
                        .unwrap_or_default();

                    let id = Offer::build_id(&market.id, &title, Some(&valid_from));

                    offers.push(Offer {
                        id,
                        market_id: market.id.clone(),
                        title,
                        subtitle,
                        overline,
                        price,
                        regular_price,
                        category: Some(category.clone()),
                        nutri_score: None,
                        valid_from: Some(valid_from.clone()),
                        valid_until: Some(valid_until.clone()),
                        images,
                        biozid: false,
                        flyer_page: None,
                    });
                }
            }
        }

        Ok(offers)
    })
}

fn build_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .context("HTTP-Client konnte nicht erstellt werden")
}

async fn fetch_markets(client: &reqwest::Client) -> Result<Vec<serde_json::Value>> {
    let resp = client
        .get(format!("{BASE_URL}/.rest/market"))
        .send()
        .await
        .context("Penny-Marktliste-Request fehlgeschlagen")?;

    if !resp.status().is_success() {
        bail!("Penny-Marktliste lieferte HTTP {}", resp.status());
    }

    let raw: serde_json::Value = resp
        .json()
        .await
        .context("Penny-Marktliste JSON parse fehlgeschlagen")?;

    raw.as_array()
        .cloned()
        .context("Penny-Marktliste ist kein JSON-Array")
}

// Alle Werte von attr="..." aus dem HTML, dedupliziert in Reihenfolge des Auftretens
fn extract_attr_values(html: &str, attr: &str) -> Vec<String> {
    let needle = format!("{attr}=\"");
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut rest = html;
    while let Some(pos) = rest.find(&needle) {
        rest = &rest[pos + needle.len()..];
        if let Some(end) = rest.find('"') {
            let val = &rest[..end];
            if !val.is_empty() && seen.insert(val.to_string()) {
                out.push(val.to_string());
            }
            rest = &rest[end..];
        } else {
            break;
        }
    }
    out
}

// "2026-29" -> (Montag, Sonntag) der ISO-Woche als YYYY-MM-DD
fn week_dates(week: &str) -> Result<(String, String)> {
    let (year, wk) = week
        .split_once('-')
        .with_context(|| format!("Unerwartetes Wochenformat: {week}"))?;
    let year: i32 = year.parse().with_context(|| format!("Unerwartetes Wochenformat: {week}"))?;
    let wk: u32 = wk.parse().with_context(|| format!("Unerwartetes Wochenformat: {week}"))?;

    let monday = chrono::NaiveDate::from_isoywd_opt(year, wk, chrono::Weekday::Mon)
        .with_context(|| format!("Ungültige ISO-Woche: {week}"))?;
    let sunday = monday + chrono::Days::new(6);
    Ok((monday.format("%Y-%m-%d").to_string(), sunday.format("%Y-%m-%d").to_string()))
}

fn json_price(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().replace(',', ".").parse::<f64>().ok()))
}

// main() ist synchron — eigener Runtime, damit die Aufrufer-API wie bei rewe.rs sync bleibt.
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
    fn live_fetch_offers_for_example_zip() {
        let market = find_market("10115").expect("Markt für 10115");
        println!("Markt: {} ({})", market.name, market.id);

        let offers = fetch_offers(&market).expect("Angebote");
        println!("{} Angebote", offers.len());
        for o in offers.iter().take(15) {
            println!(
                "- {} | {:?} | {:?} € (statt {:?}) | {:?} | {} bis {}",
                o.title,
                o.subtitle,
                o.price,
                o.regular_price,
                o.category,
                o.valid_from.as_deref().unwrap_or("?"),
                o.valid_until.as_deref().unwrap_or("?"),
            );
        }
        assert!(!offers.is_empty(), "Keine Angebote geparst");
    }
}
