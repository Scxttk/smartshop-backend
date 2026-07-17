use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::db;
use crate::models::{Market, Offer};
use crate::push::{self, PushConfig, PushOptions};
use crate::stores::save_offers;

/// Ergebnis des Scrapens einer Region: pro Kette (Anzeigename wie in
/// `Store::chain()`) Markt + Angebote, `Ok(None)` wenn die Kette laut
/// Store-Finder keine Filiale im Umkreis der PLZ hat, oder ein Fehler.
pub type FetchResult = Vec<(String, Result<Option<(Market, Vec<Offer>)>>)>;

/// Scrape-Funktion, die für eine PLZ alle Ketten abruft. In Produktion eine
/// Closure über `Store::ALL` + `scrape_store`; in Tests ein Stub ohne Netz.
pub type Fetcher<'a> = dyn Fn(&str) -> FetchResult + 'a;

pub struct SyncOptions {
    pub db_path: String,
    pub dry_run: bool,
    /// Höchstens so viele Regionen pro Lauf syncen; weitere werden geloggt
    /// und übersprungen.
    pub max_regions: usize,
}

#[derive(Debug, Deserialize)]
pub struct Region {
    pub plz: String,
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

fn auth(
    cfg: &PushConfig,
    req: reqwest::blocking::RequestBuilder,
) -> reqwest::blocking::RequestBuilder {
    req.header("apikey", &cfg.api_key)
        .header("Authorization", format!("Bearer {}", cfg.api_key))
}

/// Aktive Regionen aus Supabase laden, älteste Anfrage zuerst.
pub fn fetch_regions(cfg: &PushConfig) -> Result<Vec<Region>> {
    let client = reqwest::blocking::Client::new();
    let resp = auth(cfg, client.get(format!("{}/rest/v1/regions", cfg.base_url)))
        .query(&[
            ("select", "plz"),
            ("active", "eq.true"),
            ("order", "requested_at"),
        ])
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        let excerpt: String = body.chars().take(300).collect();
        bail!("Regionen laden fehlgeschlagen (HTTP {status}): {excerpt}");
    }
    let regions: Vec<Region> = resp.json().context("Regionen-Antwort ist kein gültiges JSON")?;
    Ok(regions)
}

/// Gefundene Märkte einer Region nach `public.markets` upserten.
fn upsert_markets(cfg: &PushConfig, rows: &[serde_json::Value]) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let resp = auth(cfg, client.post(format!("{}/rest/v1/markets", cfg.base_url)))
        .query(&[("on_conflict", "chain,plz")])
        .header("Prefer", "resolution=merge-duplicates")
        .json(rows)
        .send()
        .with_context(|| format!("Supabase nicht erreichbar ({})", cfg.base_url))?;
    check_response(&format!("Upsert von {} Märkten", rows.len()), resp)
}

/// Eine Region komplett syncen: Ketten scrapen, Märkte upserten, Angebote
/// pushen. Die lokale offers-Tabelle wird vorher geleert, damit `push` nur
/// Angebote dieser Region hochlädt.
fn sync_region(opts: &SyncOptions, cfg: &PushConfig, fetcher: &Fetcher, plz: &str) -> Result<()> {
    {
        let conn = db::open(&opts.db_path)?;
        conn.execute("DELETE FROM offers", [])
            .context("Lokale offers-Tabelle konnte nicht geleert werden")?;
    }

    struct Row {
        chain: String,
        market: String,
        result: std::result::Result<usize, String>,
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut market_rows: Vec<serde_json::Value> = Vec::new();

    for (chain, result) in fetcher(plz) {
        match result {
            Ok(None) => {
                // Kette hat keine Filiale im Umkreis — nicht registrieren,
                // zählt weder als Erfolg noch als Fehler.
                println!("[{plz}] {chain}: keine Filiale in der Nähe — übersprungen.");
            }
            Ok(Some((market, offers))) => {
                println!("[{plz}] {chain}: {} Angebote gefunden.", offers.len());
                match save_offers(&opts.db_path, &market, &offers) {
                    Ok(()) => {
                        market_rows.push(serde_json::json!({
                            "chain": chain,
                            "branch_name": market.name,
                            "market_id": market.id,
                            "plz": plz,
                            "lat": market.lat,
                            "lon": market.lon,
                            "updated_at": chrono::Utc::now().to_rfc3339(),
                        }));
                        rows.push(Row { chain, market: market.name, result: Ok(offers.len()) });
                    }
                    Err(e) => {
                        eprintln!("[{plz}] {chain}: Speichern fehlgeschlagen: {e:#}");
                        rows.push(Row { chain, market: market.name, result: Err(format!("{e:#}")) });
                    }
                }
            }
            Err(e) => {
                eprintln!("[{plz}] {chain}: Fehler: {e:#}");
                rows.push(Row { chain, market: "-".to_string(), result: Err(format!("{e:#}")) });
            }
        }
    }

    println!("Zusammenfassung (Region {plz}):");
    println!("  {:<10} {:<28} {}", "Markt", "Filiale", "Ergebnis");
    for row in &rows {
        let result = match &row.result {
            Ok(n) => format!("{n} Angebote"),
            Err(e) => {
                let first = e.lines().next().unwrap_or("Fehler");
                format!("FEHLER: {first}")
            }
        };
        println!("  {:<10} {:<28} {}", row.chain, row.market, result);
    }

    if !rows.iter().any(|r| r.result.is_ok()) {
        bail!("Region {plz}: keine Kette erfolgreich abgerufen.");
    }

    if opts.dry_run {
        println!("[{plz}] Dry-Run — Märkte werden nicht hochgeladen.");
    } else if !market_rows.is_empty() {
        upsert_markets(cfg, &market_rows)?;
        println!("[{plz}] {} Märkte nach Supabase gemeldet.", market_rows.len());
    }

    push::run(
        &PushOptions {
            db_path: opts.db_path.clone(),
            chain: None,
            region: Some(plz.to_string()),
            dry_run: opts.dry_run,
            mirror_images: true,
        },
        Some(cfg),
    )
}

/// Alle aktiven Regionen aus Supabase syncen. Fehler einzelner Regionen
/// brechen den Lauf nicht ab; Exit-Fehler nur, wenn ALLE Regionen scheitern.
pub fn run(opts: &SyncOptions, cfg: Option<&PushConfig>, fetcher: &Fetcher) -> Result<()> {
    let cfg = match cfg {
        Some(c) => c,
        None => &push::config_from_env()?,
    };

    let regions = fetch_regions(cfg)?;
    if regions.is_empty() {
        bail!("Keine aktiven Regionen in Supabase gefunden.");
    }

    if regions.len() > opts.max_regions {
        let skipped: Vec<&str> = regions[opts.max_regions..]
            .iter()
            .map(|r| r.plz.as_str())
            .collect();
        println!(
            "{} Regionen angefordert, Limit {} — übersprungen: {}",
            regions.len(),
            opts.max_regions,
            skipped.join(", ")
        );
    }

    let selected = &regions[..regions.len().min(opts.max_regions)];
    println!(
        "Synce {} Region(en): {}",
        selected.len(),
        selected.iter().map(|r| r.plz.as_str()).collect::<Vec<_>>().join(", ")
    );

    let mut failures: Vec<(String, String)> = Vec::new();
    for region in selected {
        println!("\n=== Region {} ===", region.plz);
        if let Err(e) = sync_region(opts, cfg, fetcher, &region.plz) {
            eprintln!("Region {} fehlgeschlagen: {e:#}", region.plz);
            failures.push((region.plz.clone(), format!("{e:#}")));
        }
    }

    let ok = selected.len() - failures.len();
    println!("\nFertig: {ok}/{} Region(en) erfolgreich gesynct.", selected.len());
    if ok == 0 {
        bail!(
            "Alle {} Region(en) fehlgeschlagen: {}",
            selected.len(),
            failures
                .iter()
                .map(|(plz, e)| format!("{plz} ({e})"))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    Ok(())
}
