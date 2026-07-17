use anyhow::{Context, Result, bail};
use std::collections::HashSet;

use crate::models::{Market, Offer};
use crate::scrapers::util;

// Lidl-Angebote über die öffentliche Such-API des Onlineshops (kein Login nötig).
//
//   GET https://www.lidl.de/q/api/search?assortment=DE&locale=de_DE&version=v2.0.0
//       &store=1&fetchsize=...&offset=...
//
// `store=1` ist der "In der Filiale"-Facet-Filter und liefert die aktuellen
// Filialangebote als Produkt-Tiles (items[].gridbox.data). Die früheren
// Kampagnenseiten (/c/billiger-montag/...) leiten inzwischen auf die
// Online-Prospekte um und sind als Datenquelle tot; die Lidl-Plus-API ist
// authentifiziert. Wichtig: Header "Accept: application/json" mitsenden,
// sonst antwortet der Endpoint mit HTTP 406.
//
// Lidl-Angebote gelten bundesweit, nicht pro Filiale. find_market liefert
// deshalb unabhängig von der PLZ einen synthetischen National-Markt.

const SEARCH_URL: &str = "https://www.lidl.de/q/api/search";
const PAGE_SIZE: usize = 200;
const MAX_OFFERS: usize = 2000;

pub fn find_market(_zip: &str) -> Result<Market> {
    // Angebote sind national — die PLZ hat keinen Einfluss.
    Ok(Market { id: "LIDL_DE".to_string(), name: "Lidl Deutschland".to_string() })
}

pub fn fetch_offers(market: &Market) -> Result<Vec<Offer>> {
    block_on(async {
        let client = util::async_client()?;

        let mut offers = Vec::new();
        let mut seen = HashSet::new();
        let mut offset = 0usize;

        loop {
            let url = format!(
                "{SEARCH_URL}?assortment=DE&locale=de_DE&version=v2.0.0&store=1&fetchsize={PAGE_SIZE}&offset={offset}"
            );

            util::polite_pause(&url);
            let resp = client
                .get(&url)
                .header("Accept", "application/json, text/plain, */*")
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

            let num_found = raw.get("numFound").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let items = raw
                .get("items")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            if items.is_empty() {
                break;
            }

            for item in &items {
                if let Some(offer) = parse_tile(item, &market.id) {
                    if seen.insert(offer.id.clone()) {
                        offers.push(offer);
                    }
                }
            }

            offset += items.len();
            if offset >= num_found.min(MAX_OFFERS) {
                break;
            }
        }

        Ok(offers)
    })
}

// Ein Produkt-Tile defensiv in ein Offer übersetzen; bei fehlenden
// Pflichtfeldern None (Tile wird übersprungen).
pub fn parse_tile(item: &serde_json::Value, market_id: &str) -> Option<Offer> {
    let data = item.get("gridbox")?.get("data")?;

    let title = data
        .get("fullTitle")
        .or_else(|| data.get("title"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();

    // Lidl-Plus-exklusive Angebote tragen ihren Preis nicht in data.price
    // (dort steht nur eine leere Hülle mit currencyCode), sondern in
    // data.lidlPlus[0].price — daher der Fallback.
    let price_obj = data
        .get("price")
        .filter(|p| p.get("price").is_some())
        .or_else(|| item.pointer("/gridbox/data/lidlPlus/0/price"));
    let price = price_obj.and_then(|p| p.get("price")).and_then(|v| v.as_f64());
    let regular_price = price_obj
        .and_then(|p| p.get("oldPrice"))
        .and_then(|v| v.as_f64())
        .or_else(|| {
            price_obj?
                .get("discount")?
                .get("deletedPrice")
                .and_then(|v| v.as_f64())
        });

    // Verpackungsgröße als Untertitel, z. B. "0.75 l"
    let subtitle = price_obj
        .and_then(|p| p.get("packaging"))
        .and_then(|pack| {
            let amount = pack.get("amount")?.as_f64()?;
            let unit = pack.get("unit")?.as_str()?;
            Some(format!("{amount} {unit}"))
        })
        .or_else(|| {
            // Lidl-Plus-Preise haben nur einen Freitext ("Je Stück")
            price_obj?
                .get("packaging")?
                .get("text")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .or_else(|| {
            price_obj?
                .get("basePrice")?
                .get("text")
                .and_then(|v| v.as_str())
                .map(String::from)
        });

    let overline = data
        .get("brand")
        .and_then(|b| b.get("name"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);

    // "Kategorien/Essen & Trinken/..." -> letztes Segment
    let category = data
        .get("category")
        .and_then(|v| v.as_str())
        .and_then(|s| s.rsplit('/').next())
        .filter(|s| !s.is_empty() && *s != "Kategorien")
        .map(String::from);

    // "2026-07-19T22:00" -> "2026-07-19"
    let valid_from = price_obj
        .and_then(|p| p.get("startDate"))
        .and_then(|v| v.as_str())
        .map(|s| s.chars().take(10).collect::<String>());
    let valid_until = price_obj
        .and_then(|p| p.get("endDate"))
        .and_then(|v| v.as_str())
        .or_else(|| data.get("storeEndDate").and_then(|v| v.as_str()))
        .map(|s| s.chars().take(10).collect::<String>());

    let images: Vec<String> = data
        .get("image")
        .and_then(|v| v.as_str())
        .map(|s| vec![s.to_string()])
        .unwrap_or_default();

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
    #[ignore = "Live-Test gegen lidl.de: cargo test lidl -- --ignored --nocapture"]
    fn live_fetch_offers() {
        let market = find_market("10115").expect("Markt");
        println!("Markt: {} ({})", market.name, market.id);

        let offers = fetch_offers(&market).expect("Angebote");
        println!("{} Angebote", offers.len());
        for o in offers.iter().take(5) {
            println!(
                "- {} | {:?} | {:?} € (statt {:?}) | {:?} | bis {:?}",
                o.title, o.subtitle, o.price, o.regular_price, o.category, o.valid_until
            );
        }
        assert!(offers.len() >= 100, "Erwartet dreistellige Angebotszahl, war {}", offers.len());
    }
}
