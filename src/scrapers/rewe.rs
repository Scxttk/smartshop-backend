use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

use crate::models::{Market, Offer};

// Calls the `rewerse` Go CLI (https://github.com/ByteSizedMarius/rewerse-engineering)
// Requires: cert.pem and private.key in the working directory (extracted once from Rewe APK)

fn rewerse_cmd(cert: &str, key: &str) -> Command {
    let mut cmd = Command::new("rewerse");
    cmd.arg("-cert").arg(cert).arg("-key").arg(key);
    cmd
}

pub fn find_market(zip: &str, cert: &str, key: &str) -> Result<Market> {
    check_certs(cert, key)?;
    let output = rewerse_cmd(cert, key)
        .args(["markets", "search", "-query", zip, "-json"])
        .output()
        .context("rewerse CLI nicht gefunden — bitte installieren (siehe README)")?;

    if !output.status.success() {
        bail!(
            "[REWE] Markt-Lookup fehlgeschlagen (rewerse markets search -query {zip}):\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let raw: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("[REWE] Markt-Lookup JSON parsen fehlgeschlagen (rewerse markets search)")?;

    let markets = raw
        .as_array()
        .or_else(|| raw.get("markets").and_then(|v| v.as_array()))
        .context("Keine Märkte in der Antwort")?;

    let first = markets.first().context("Kein Markt für diese PLZ gefunden")?;
    let id = first
        .get("marketId")
        .or_else(|| first.get("id"))
        .and_then(|v| v.as_str())
        .context("marketId fehlt")?;
    let name = first
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("REWE");

    Ok(Market::new(id, name))
}

pub fn fetch_offers(market: &Market, cert: &str, key: &str) -> Result<Vec<Offer>> {
    check_certs(cert, key)?;
    let output = rewerse_cmd(cert, key)
        .args(["discounts", "-market", &market.id, "-json"])
        .output()
        .context("rewerse CLI nicht gefunden")?;

    if !output.status.success() {
        bail!(
            "[REWE] Angebote laden fehlgeschlagen (rewerse discounts -market {}):\n{}",
            market.id,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let raw: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("[REWE] Angebote JSON parsen fehlgeschlagen (rewerse discounts)")?;

    parse_offers(raw, &market.id)
}

fn check_certs(cert: &str, key: &str) -> Result<()> {
    if !Path::new(cert).exists() {
        bail!("Zertifikat nicht gefunden: {cert}\nBitte Einrichtung in README befolgen.");
    }
    if !Path::new(key).exists() {
        bail!("Private Key nicht gefunden: {key}\nBitte Einrichtung in README befolgen.");
    }
    Ok(())
}

pub fn parse_offers(raw: serde_json::Value, market_id: &str) -> Result<Vec<Offer>> {
    let mut offers = Vec::new();

    // rewerse returns: { "current": { "fromDate", "untilDate", "categories": [...] }, "next": {...} }
    for week_key in &["current", "next"] {
        let Some(week) = raw.get(week_key) else { continue };
        let from = week.get("fromDate").and_then(|v| v.as_str()).map(String::from);
        let until = week.get("untilDate").and_then(|v| v.as_str()).map(String::from);

        let categories = week
            .get("categories")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for cat in &categories {
            let category = cat.get("title").and_then(|v| v.as_str()).map(String::from);
            let raw_offers = cat
                .get("offers")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            for raw_offer in &raw_offers {
                let title = match raw_offer.get("title").and_then(|v| v.as_str()) {
                    Some(t) => t.to_string(),
                    None => continue,
                };

                let subtitle = raw_offer.get("subtitle").and_then(|v| v.as_str()).map(String::from);
                let overline = raw_offer.get("overline").and_then(|v| v.as_str()).map(String::from);
                let biozid = raw_offer.get("biozid").and_then(|v| v.as_bool()).unwrap_or(false);

                // rewerse returns prices already parsed as floats
                let price = raw_offer
                    .get("priceData")
                    .and_then(|p| p.get("price"))
                    .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(parse_price_str)));
                let regular_price = raw_offer
                    .get("priceData")
                    .and_then(|p| p.get("regularPrice"))
                    .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(parse_price_str)));

                let nutri_score = raw_offer
                    .get("detail")
                    .and_then(|d| d.get("nutriScore"))
                    .and_then(|v| v.as_str())
                    .map(String::from);

                let cat_title = raw_offer
                    .get("rawValues")
                    .and_then(|rv| rv.get("categoryTitle"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or_else(|| category.clone());

                let flyer_page = raw_offer
                    .get("rawValues")
                    .and_then(|rv| rv.get("flyerPage"))
                    .and_then(|v| v.as_i64());

                let images: Vec<String> = raw_offer
                    .get("images")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).map(String::from).collect())
                    .unwrap_or_default();

                let id = Offer::build_id(market_id, &title, from.as_deref());

                offers.push(Offer {
                    id,
                    market_id: market_id.to_string(),
                    title,
                    subtitle,
                    overline,
                    price,
                    regular_price,
                    category: cat_title,
                    nutri_score,
                    valid_from: from.clone(),
                    valid_until: until.clone(),
                    images,
                    biozid,
                    flyer_page,
                });
            }
        }
    }

    Ok(offers)
}

fn parse_price_str(s: &str) -> Option<f64> {
    s.trim().replace('.', "").replace(',', ".").parse::<f64>().ok()
}
