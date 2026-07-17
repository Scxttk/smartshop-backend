//! Upload gespeicherter Angebote in die Supabase-Tabelle `public.offers`
//! (PostgREST-API). Ersetzt den Python-Uploader `supabase_uploader.py`:
//! Upsert mit on_conflict=market,product,valid_from,region, Batches à 100,
//! vorheriges Löschen veralteter Wochen pro Markt, Region-Cache in
//! `public.regions`.

use std::collections::{BTreeMap, HashSet};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::models::{Market, Offer};
use crate::{db, enrich, units};

pub const BATCH_SIZE: usize = 100;

/// Zeile im Supabase-Schema (schema.sql + migration_v2.sql + migration_regions.sql).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SupabaseRow {
    pub market: String,
    pub product: String,
    pub price: f64,
    pub regular_price: Option<f64>,
    pub unit: String,
    pub category: Option<String>,
    pub emoji: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub base_price: Option<f64>,
    pub base_unit: Option<String>,
    pub brand: Option<String>,
    pub ean: Option<String>,
    pub source: String,
    pub region: Option<String>,
}

/// Ketten-Anzeigename für einen gespeicherten Markt. Die Zuordnung läuft über
/// Markt-ID und Filialnamen; Filialen ohne erkennbare Kette liefern None und
/// werden beim Push übersprungen.
pub fn chain_for(market: &Market) -> Option<&'static str> {
    let hay = format!("{} {}", market.id, market.name).to_lowercase();
    if hay.contains("aldi") {
        if hay.contains("nord") {
            return Some("ALDI Nord");
        }
        if hay.contains("süd") || hay.contains("sued") {
            return Some("ALDI SÜD");
        }
    }
    let chains = [
        ("rewe", "REWE"),
        ("penny", "Penny"),
        ("kaufland", "Kaufland"),
        ("lidl", "Lidl"),
        ("netto", "Netto"),
        ("edeka", "EDEKA"),
        // EDEKA-Vertriebsmarken tragen "edeka" nicht immer im Namen
        ("e center", "EDEKA"),
        ("e-center", "EDEKA"),
        ("e neukauf", "EDEKA"),
        ("e aktiv", "EDEKA"),
        ("e xpress", "EDEKA"),
        ("marktkauf", "EDEKA"),
        ("nah & gut", "EDEKA"),
        ("nah und gut", "EDEKA"),
    ];
    chains.iter().find(|(n, _)| hay.contains(n)).map(|(_, c)| *c)
}

// Produktname: Titel, plus Untertitel wenn er echte Zusatzinfo trägt
// (kein reiner Mengen-Text, nicht leer, nicht schon im Titel enthalten).
fn product_name(offer: &Offer) -> String {
    match &offer.subtitle {
        Some(sub)
            if !sub.is_empty()
                && !is_pure_quantity(sub)
                && !offer.title.to_lowercase().contains(&sub.to_lowercase()) =>
        {
            format!("{} {}", offer.title, sub)
        }
        _ => offer.title.clone(),
    }
}

// Ein Untertitel ist nur dann reiner Mengen-Text, wenn er außer Zahlen nichts
// als Einheiten-/Verpackungswörter enthält ("je 250-g-Packg."). Texte wie
// "Rispentomaten, 500-g-Schale" tragen den Produktnamen und müssen erhalten
// bleiben — units::parse_quantity findet auch dort eine Menge und reicht als
// Kriterium deshalb nicht.
fn is_pure_quantity(text: &str) -> bool {
    const FILLER: &[&str] = &[
        "je", "ca", "x", "g", "kg", "mg", "ml", "cl", "l", "stück", "stk", "er", "packg",
        "packung", "pack", "dose", "flasche", "beutel", "schale", "netz", "becher", "glas",
        "tafel", "riegel", "rolle", "portion",
    ];
    text.to_lowercase()
        .split(|c: char| !c.is_alphabetic())
        .filter(|t| !t.is_empty())
        .all(|t| FILLER.contains(&t))
}

/// Lokales Angebot in eine Supabase-Zeile mappen. None bei Angeboten ohne
/// Preis — die kann die App nicht anzeigen.
pub fn map_offer(offer: &Offer, chain: &str, region: Option<&str>) -> Option<SupabaseRow> {
    let price = offer.price?;
    let enriched =
        enrich::enrich(&offer.title, offer.subtitle.as_deref(), offer.category.as_deref());
    let unit_price = units::derive_unit_price(
        offer.price,
        &[offer.subtitle.as_deref(), offer.overline.as_deref(), Some(&offer.title)],
    );
    Some(SupabaseRow {
        market: chain.to_string(),
        product: product_name(offer),
        price,
        regular_price: offer.regular_price,
        // Mengen-Untertitel ("je 12 x 1 l") wandert ins unit-Feld — sonst
        // wirkt ein Multipack-Preis in der App wie ein Einzelpreis.
        unit: match &offer.subtitle {
            Some(s) if !s.is_empty() && is_pure_quantity(s) => s.clone(),
            _ => "Stück".to_string(),
        },
        category: Some(enriched.category.to_string()),
        emoji: Some(enriched.emoji.to_string()),
        valid_from: offer.valid_from.clone(),
        valid_until: offer.valid_until.clone(),
        base_price: unit_price.map(|up| (up.eur * 100.0).round() / 100.0),
        base_unit: unit_price.map(|up| up.unit.label().to_string()),
        brand: None,
        ean: None,
        source: "smartshop-rust".to_string(),
        region: region.map(String::from),
    })
}

/// Duplikate auf dem Upsert-Schlüssel (market, product, valid_from, region)
/// entfernen; der erste Treffer gewinnt.
pub fn dedupe_rows(rows: Vec<SupabaseRow>) -> Vec<SupabaseRow> {
    let mut seen = HashSet::new();
    rows.into_iter()
        .filter(|r| {
            seen.insert((
                r.market.clone(),
                r.product.clone(),
                r.valid_from.clone(),
                r.region.clone(),
            ))
        })
        .collect()
}

/// Zeile für `public.price_history` (migration_v7): nur die Preis-relevanten
/// Spalten einer SupabaseRow; `recorded_at` setzt die Datenbank.
#[derive(Debug, Serialize)]
struct HistoryRow<'a> {
    market: &'a str,
    product: &'a str,
    region: Option<&'a str>,
    price: f64,
    regular_price: Option<f64>,
    base_price: Option<f64>,
    base_unit: Option<&'a str>,
    unit: &'a str,
    category: Option<&'a str>,
    valid_from: Option<&'a str>,
    valid_until: Option<&'a str>,
}

impl<'a> From<&'a SupabaseRow> for HistoryRow<'a> {
    fn from(r: &'a SupabaseRow) -> Self {
        HistoryRow {
            market: &r.market,
            product: &r.product,
            region: r.region.as_deref(),
            price: r.price,
            regular_price: r.regular_price,
            base_price: r.base_price,
            base_unit: r.base_unit.as_deref(),
            unit: &r.unit,
            category: r.category.as_deref(),
            valid_from: r.valid_from.as_deref(),
            valid_until: r.valid_until.as_deref(),
        }
    }
}

pub struct PushConfig {
    pub base_url: String,
    pub api_key: String,
}

pub fn config_from_env() -> Result<PushConfig> {
    let base_url = std::env::var("SUPABASE_URL")
        .context("Umgebungsvariable SUPABASE_URL fehlt (z. B. https://xyz.supabase.co)")?;
    let api_key = std::env::var("SUPABASE_SERVICE_KEY")
        .context("Umgebungsvariable SUPABASE_SERVICE_KEY fehlt (Service-Role-Key, nicht der anon key)")?;
    Ok(PushConfig { base_url: base_url.trim_end_matches('/').to_string(), api_key })
}

pub struct PushOptions {
    pub db_path: String,
    /// Nur diese Kette pushen (Anzeigename, z. B. "REWE"); None = alle.
    pub chain: Option<String>,
    /// PLZ, aus der die Angebote stammen. Pflicht außer bei --dry-run.
    pub region: Option<String>,
    pub dry_run: bool,
}

/// Angebote aus der lokalen DB laden und pro Kette gruppieren (gemappt,
/// dedupliziert; Angebote ohne Preis werden mitgezählt, aber übersprungen).
fn load_grouped(opts: &PushOptions) -> Result<BTreeMap<&'static str, (Vec<SupabaseRow>, usize)>> {
    let conn = db::open(&opts.db_path)?;
    let markets = db::markets(&conn)?;
    let offers = db::export_offers(&conn, None)?;

    let chain_by_market: BTreeMap<&str, &'static str> = markets
        .iter()
        .filter_map(|m| chain_for(m).map(|c| (m.id.as_str(), c)))
        .collect();

    let mut groups: BTreeMap<&'static str, (Vec<Offer>, usize)> = BTreeMap::new();
    for offer in offers {
        let Some(&chain) = chain_by_market.get(offer.market_id.as_str()) else {
            continue;
        };
        if let Some(want) = &opts.chain {
            if chain != want {
                continue;
            }
        }
        let entry = groups.entry(chain).or_default();
        if offer.price.is_none() {
            entry.1 += 1;
        } else {
            entry.0.push(offer);
        }
    }

    Ok(groups
        .into_iter()
        .map(|(chain, (offers, skipped))| {
            let rows = offers
                .iter()
                .filter_map(|o| map_offer(o, chain, opts.region.as_deref()))
                .collect();
            (chain, (dedupe_rows(rows), skipped))
        })
        .collect())
}

fn dry_run_report(groups: &BTreeMap<&'static str, (Vec<SupabaseRow>, usize)>) -> Result<()> {
    println!("Dry-Run — es wird nichts hochgeladen.");
    for (chain, (rows, skipped)) in groups {
        println!("  [{chain}] {} Zeilen ({skipped} ohne Preis übersprungen)", rows.len());
    }
    let samples: Vec<&SupabaseRow> = groups.values().flat_map(|(r, _)| r).take(3).collect();
    if !samples.is_empty() {
        println!("Beispiel-Zeilen:");
        println!("{}", serde_json::to_string_pretty(&samples)?);
    }
    Ok(())
}

fn check_response(what: &str, resp: reqwest::blocking::Response) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().unwrap_or_default();
    let excerpt: String = body.chars().take(300).collect();
    bail!("{what} fehlgeschlagen (HTTP {status}): {excerpt}");
}

/// Alle gepushten Zeilen zusätzlich in `public.price_history` upserten
/// (Wochen-Schnappschuss für Preisverlaufs-Charts). Zeilen ohne Preis kommen
/// hier nie an — map_offer filtert sie bereits. Liefert die Zeilenzahl.
fn push_history(
    client: &reqwest::blocking::Client,
    cfg: &PushConfig,
    rows: &[&SupabaseRow],
) -> Result<usize> {
    let url = format!("{}/rest/v1/price_history", cfg.base_url);
    for batch in rows.chunks(BATCH_SIZE) {
        let payload: Vec<HistoryRow> = batch.iter().map(|r| HistoryRow::from(*r)).collect();
        let resp = client
            .post(&url)
            .header("apikey", &cfg.api_key)
            .header("Authorization", format!("Bearer {}", cfg.api_key))
            .query(&[("on_conflict", "market,product,region,valid_from")])
            .header("Prefer", "resolution=merge-duplicates")
            .json(&payload)
            .send()
            .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
        check_response(&format!("Preis-Historie: Upsert von {} Zeilen", payload.len()), resp)?;
    }
    Ok(rows.len())
}

pub fn run(opts: &PushOptions, cfg: Option<&PushConfig>) -> Result<()> {
    let groups = load_grouped(opts)?;
    if groups.is_empty() {
        println!("Keine passenden Angebote in '{}' gefunden.", opts.db_path);
        return Ok(());
    }

    if opts.dry_run {
        return dry_run_report(&groups);
    }

    let Some(region) = opts.region.as_deref() else {
        bail!("--region <PLZ> ist Pflicht (außer bei --dry-run): die App filtert Angebote pro Region.");
    };
    let cfg = match cfg {
        Some(c) => c,
        None => &config_from_env()?,
    };

    let client = reqwest::blocking::Client::new();
    let offers_url = format!("{}/rest/v1/offers", cfg.base_url);
    let auth = |req: reqwest::blocking::RequestBuilder| {
        req.header("apikey", &cfg.api_key)
            .header("Authorization", format!("Bearer {}", cfg.api_key))
    };

    let mut total = 0usize;
    for (chain, (rows, skipped)) in &groups {
        if rows.is_empty() {
            println!("  [{chain}] Keine Angebote mit Preis — übersprungen.");
            continue;
        }

        // Veraltete Wochen dieses Markts löschen — nur wenn neue Daten mit
        // Gültigkeitsdatum vorliegen.
        if let Some(current) = rows.iter().filter_map(|r| r.valid_from.as_deref()).max() {
            let resp = auth(client.delete(&offers_url))
                .query(&[
                    ("market", format!("eq.{chain}")),
                    ("valid_from", format!("lt.{current}")),
                ])
                .send()
                .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
            check_response(&format!("[{chain}] Löschen veralteter Angebote"), resp)?;
        }

        for batch in rows.chunks(BATCH_SIZE) {
            let resp = auth(client.post(&offers_url))
                .query(&[("on_conflict", "market,product,valid_from,region")])
                .header("Prefer", "resolution=merge-duplicates")
                .json(batch)
                .send()
                .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
            check_response(&format!("[{chain}] Upsert von {} Angeboten", batch.len()), resp)?;
        }
        total += rows.len();
        println!("  [{chain}] {} Angebote hochgeladen ({skipped} ohne Preis übersprungen).", rows.len());
    }

    // Preis-Historie best-effort mitschreiben: Fehler (z. B. Tabelle noch
    // nicht migriert) warnen nur und lassen den Offers-Push erfolgreich.
    let history_rows: Vec<&SupabaseRow> = groups.values().flat_map(|(r, _)| r).collect();
    if !history_rows.is_empty() {
        match push_history(&client, cfg, &history_rows) {
            Ok(n) => println!("Preis-Historie: {n} Zeilen upsertet."),
            Err(e) => eprintln!("WARNUNG: Preis-Historie fehlgeschlagen: {e:#}"),
        }
    }

    // Region-Cache aktualisieren: diese PLZ wurde soeben gesynct.
    let resp = auth(client.post(format!("{}/rest/v1/regions", cfg.base_url)))
        .query(&[("on_conflict", "plz")])
        .header("Prefer", "resolution=merge-duplicates")
        .json(&serde_json::json!([{
            "plz": region,
            "last_synced": chrono::Utc::now().to_rfc3339(),
        }]))
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    check_response(&format!("Region {region} eintragen"), resp)?;

    println!("Fertig: {total} Angebote nach Supabase gepusht (Region {region}).");
    Ok(())
}
