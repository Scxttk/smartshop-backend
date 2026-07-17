use anyhow::{Context, Result, bail};

use crate::scrapers::util::curl_get;
use scraper::{ElementRef, Html, Selector};
use std::collections::HashSet;

use crate::models::{Market, Offer};

// Netto Marken-Discount über netto-online.de (Intershop, Akamai-geschützt).
//
// Filialsuche (öffentliches JSON, kein Token nötig):
//   GET /INTERSHOP/web/WFS/Plus-NettoDE-Site/de_DE/-/EUR/
//       ViewMMPStoreFinder-GetStoreByPostcode?postalcode=<PLZ>&searchradius=25
//
// Filial-Angebote sind server-seitig gerendert; die Filiale wird allein über
// das Cookie `netto_user_stores_id=<store_id>` gebunden (verifiziert 2026-07):
//   GET /filialangebote/1   Wochenangebote
//   GET /filialangebote/2   Wochenendangebote
//   GET /filialangebote/4   Freitag ist Netto-Tag
//   GET /filialangebote/5   Samstagskracher
// (Unbekannte Seiten-IDs liefern Seite 1 — Dedup fängt das ab.)
//
// Achtung Akamai: Der Bot-Schutz fingerprintet den TLS-Stack — reqwest/rustls
// wird konsequent mit HTTP 403 geblockt, curl mit vollem Browser-Header-Satz
// (User-Agent + Accept + Sec-Fetch-*) kommt durch (verifiziert 2026-07).
// Deshalb laufen die Requests über util::curl_get (System-curl).

const BASE: &str = "https://www.netto-online.de";
const STORE_FINDER_PATH: &str =
    "/INTERSHOP/web/WFS/Plus-NettoDE-Site/de_DE/-/EUR/ViewMMPStoreFinder-GetStoreByPostcode";
const OFFER_PAGES: &[u32] = &[1, 2, 4, 5];

pub fn find_market(zip: &str) -> Result<Market> {
    let url = format!("{BASE}{STORE_FINDER_PATH}?postalcode={zip}&searchradius=25");
    let body = curl_get(
        &url,
        &[
            ("Accept", "application/json, text/javascript, */*; q=0.01"),
            ("X-Requested-With", "XMLHttpRequest"),
            ("Referer", "https://www.netto-online.de/filialangebote/"),
            ("Sec-Fetch-Site", "same-origin"),
            ("Sec-Fetch-Mode", "cors"),
            ("Sec-Fetch-Dest", "empty"),
        ],
    )?;

    let stores: Vec<serde_json::Value> =
        serde_json::from_str(&body).context("Netto-Filialsuche JSON parse fehlgeschlagen")?;

    let store = stores
        .first()
        .with_context(|| format!("Keine Netto-Filiale für PLZ {zip} gefunden"))?;

    let id = store
        .get("store_id")
        .and_then(|v| v.as_str())
        .context("store_id fehlt in der Filialsuche-Antwort")?;
    let name = match (
        store.get("store_name").and_then(|v| v.as_str()),
        store.get("city").and_then(|v| v.as_str()),
    ) {
        (Some(n), Some(c)) => format!("{n} {c}"),
        (Some(n), None) => n.to_string(),
        _ => "Netto Marken-Discount".to_string(),
    };

    Ok(Market { id: id.to_string(), name })
}

pub fn fetch_offers(market: &Market) -> Result<Vec<Offer>> {
    let cookie = format!("netto_user_stores_id={}", market.id);

    let mut offers = Vec::new();
    let mut seen = HashSet::new();

    for page in OFFER_PAGES {
        let url = format!("{BASE}/filialangebote/{page}");
        let html = match curl_get(
            &url,
            &[
                ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"),
                ("Cookie", cookie.as_str()),
                ("Sec-Fetch-Site", "none"),
                ("Sec-Fetch-Mode", "navigate"),
                ("Sec-Fetch-Dest", "document"),
                ("Sec-Fetch-User", "?1"),
            ],
        ) {
            Ok(h) => h,
            // Einzelne Kategorieseite nicht erreichbar -> überspringen statt abbrechen.
            Err(_) => continue,
        };
        parse_page(&html, &market.id, &mut offers, &mut seen);
    }

    if offers.is_empty() {
        bail!("Keine Netto-Angebote gefunden — Seitenstruktur hat sich möglicherweise geändert");
    }
    Ok(offers)
}

pub fn parse_page(html: &str, market_id: &str, offers: &mut Vec<Offer>, seen: &mut HashSet<String>) {
    let doc = Html::parse_document(html);
    let sel_period = sel("div.offer__period");
    let sel_tile = sel("div.js-store-product-tile");
    let sel_title = sel(".tc-product-name");
    let sel_bundle = sel(".product-property__bundle-text");
    let sel_desc = sel(".product-property__description-short");
    let sel_base = sel(".product-property__base-price");
    let sel_price = sel(".product__current-price");
    let sel_strike = sel(".product__strike-price");
    let sel_img = sel("img.tc-product-image");

    // "Wochenangebote gültig von Montag, 13.07.26 - Samstag, 18.07.26"
    let period = doc
        .select(&sel_period)
        .next()
        .map(|e| e.text().collect::<String>());
    let (category, valid_from, valid_until) = match period.as_deref() {
        Some(p) => {
            let category = p.split(" gültig").next().map(|s| s.trim().to_string());
            let (f, u) = parse_period_dates(p).map_or((None, None), |(f, u)| (Some(f), Some(u)));
            (category, f, u)
        }
        None => (None, None, None),
    };

    for tile in doc.select(&sel_tile) {
        let Some(title) = text_of(tile, &sel_title) else { continue };

        let subtitle = text_of(tile, &sel_bundle);
        let overline = match (text_of(tile, &sel_desc), text_of(tile, &sel_base)) {
            (Some(d), Some(b)) => Some(format!("{d} ({b})")),
            (d, b) => d.or(b),
        };

        let price = text_of(tile, &sel_price).and_then(|s| parse_price(&s));
        // "UVP 3.99" -> 3.99
        let regular_price = text_of(tile, &sel_strike).and_then(|s| parse_price(&s));

        let images = tile
            .select(&sel_img)
            .next()
            .and_then(|img| img.value().attr("src"))
            .map(|s| vec![s.to_string()])
            .unwrap_or_default();

        let id = Offer::build_id(market_id, &title, valid_from.as_deref());
        if !seen.insert(id.clone()) {
            continue;
        }

        offers.push(Offer {
            id,
            market_id: market_id.to_string(),
            title,
            subtitle,
            overline,
            price,
            regular_price,
            category: category.clone(),
            nutri_score: None,
            valid_from: valid_from.clone(),
            valid_until: valid_until.clone(),
            images,
            biozid: false,
            flyer_page: None,
        });
    }
}

fn sel(css: &str) -> Selector {
    Selector::parse(css).expect("statischer CSS-Selektor")
}

fn text_of(el: ElementRef, selector: &Selector) -> Option<String> {
    let text: String = el.select(selector).next()?.text().collect();
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() { None } else { Some(text) }
}

// Preistexte wie "1. 79 *" (verschachtelte Spans) oder "UVP 3.99".
fn parse_price(s: &str) -> Option<f64> {
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    cleaned.parse::<f64>().ok()
}

// "... von Montag, 13.07.26 - Samstag, 18.07.26" -> ("2026-07-13", "2026-07-18")
fn parse_period_dates(s: &str) -> Option<(String, String)> {
    let mut dates = Vec::new();
    let mut nums = s.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty());
    while let (Some(d), Some(m), Some(y)) = (nums.next(), nums.next(), nums.next()) {
        if d.len() > 2 || m.len() > 2 || (y.len() != 2 && y.len() != 4) {
            continue;
        }
        let (day, month, year): (u32, u32, u32) =
            (d.parse().ok()?, m.parse().ok()?, y.parse().ok()?);
        if !(1..=31).contains(&day) || !(1..=12).contains(&month) {
            continue;
        }
        let year = if year < 100 { 2000 + year } else { year };
        dates.push(format!("{year}-{month:02}-{day:02}"));
    }
    match dates.as_slice() {
        [single] => Some((single.clone(), single.clone())),
        [from, .., until] => Some((from.clone(), until.clone())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_parsing() {
        assert_eq!(
            parse_period_dates("Wochenangebote gültig von Montag, 13.07.26 - Samstag, 18.07.26"),
            Some(("2026-07-13".to_string(), "2026-07-18".to_string()))
        );
        assert_eq!(parse_period_dates("Wochenangebote"), None);
    }

    #[test]
    fn price_parsing() {
        assert_eq!(parse_price("1. 79 *"), Some(1.79));
        assert_eq!(parse_price("UVP 3.99"), Some(3.99));
        assert_eq!(parse_price("—"), None);
    }

    /// Live-Test gegen netto-online.de: cargo test netto -- --ignored --nocapture
    #[test]
    #[ignore = "Live-Test gegen netto-online.de"]
    fn live_fetch_offers() {
        let market = find_market("01219").expect("Markt");
        println!("Markt: {} ({})", market.name, market.id);

        let offers = fetch_offers(&market).expect("Angebote");
        println!("{} Angebote", offers.len());
        for o in offers.iter().take(5) {
            println!(
                "- {} | {:?} | {:?} € (statt {:?}) | {:?} | {:?} bis {:?}",
                o.title, o.subtitle, o.price, o.regular_price, o.category, o.valid_from, o.valid_until
            );
        }
        assert!(offers.len() >= 80, "Erwartet >= 80 Angebote, war {}", offers.len());
    }
}
