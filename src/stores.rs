use anyhow::Result;
use clap::ValueEnum;

use crate::models::{Market, Offer};
use crate::{db, scrapers};

#[derive(Clone, Copy, ValueEnum)]
pub enum Store {
    Rewe,
    Penny,
    Kaufland,
    Lidl,
    Netto,
    AldiNord,
    AldiSued,
    Edeka,
}

impl Store {
    pub const ALL: [Store; 8] = [
        Store::Rewe,
        Store::Penny,
        Store::Kaufland,
        Store::Lidl,
        Store::Netto,
        Store::AldiNord,
        Store::AldiSued,
        Store::Edeka,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Store::Rewe => "Rewe",
            Store::Penny => "Penny",
            Store::Kaufland => "Kaufland",
            Store::Lidl => "Lidl",
            Store::Netto => "Netto",
            Store::AldiNord => "Aldi Nord",
            Store::AldiSued => "Aldi Süd",
            Store::Edeka => "Edeka",
        }
    }

    // Anzeigename der Kette im Supabase-Schema (Spalte `market`)
    pub fn chain(&self) -> &'static str {
        match self {
            Store::Rewe => "REWE",
            Store::Penny => "Penny",
            Store::Kaufland => "Kaufland",
            Store::Lidl => "Lidl",
            Store::Netto => "Netto",
            Store::AldiNord => "ALDI Nord",
            Store::AldiSued => "ALDI SÜD",
            Store::Edeka => "EDEKA",
        }
    }
}

/// Angebotsquelle für Lidl. **Standard ist die LLM-Prospekt-Pipeline**
/// (`lidl_prospekt.rs`), die die echten Filial-Wochenangebote (Frische,
/// Fleisch, Molkerei, Backshop) direkt aus dem offiziellen Prospekt liest —
/// die einzige Quelle, die diese strukturiert liefert. Der marktguru-Scraper
/// (`lidl.rs`) ist nur noch Notausgang und wird mit `LIDL_SOURCE=marktguru`
/// (auch `mg`) erzwungen (z. B. wenn kein GITHUB_MODELS_TOKEN gesetzt ist).
///
/// Runtime-Schalter statt Cargo-Feature, damit derselbe Build beide Quellen
/// fahren kann (Fallback, Vergleich) und der Schalter an genau einer Stelle
/// (`scrape_store`) greift — automatisch in `fetch`, `fetch_all` und im
/// Sync-Fetcher.
fn lidl_use_prospekt() -> bool {
    !matches!(
        std::env::var("LIDL_SOURCE").ok().as_deref(),
        Some("marktguru") | Some("mg")
    )
}

/// Obergrenze an Vision-Calls (= gefilterten Angebotsseiten) pro Prospekt-Lauf,
/// überschreibbar via `LIDL_PROSPEKT_MAX_PAGES`. `lidl_prospekt::fetch_offers`
/// kappt zusätzlich auf sein internes Hard-Limit.
fn lidl_prospekt_max_pages() -> usize {
    std::env::var("LIDL_PROSPEKT_MAX_PAGES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30)
}

/// None: die Kette hat laut Store-Finder keine Filiale im Umkreis der PLZ
/// (nur bei Lidl/ALDI möglich — die übrigen Finder scheitern dann mit Err).
pub fn scrape_store(
    store: Store,
    zip: &str,
    cert: &str,
    key: &str,
) -> Result<Option<(Market, Vec<Offer>)>> {
    println!("Suche {}-Markt für PLZ {zip}...", store.label());
    let market = match store {
        Store::Rewe => scrapers::rewe::find_market(zip, cert, key)?,
        Store::Penny => scrapers::penny::find_market(zip)?,
        Store::Kaufland => scrapers::kaufland::find_market(zip)?,
        Store::Netto => scrapers::netto::find_market(zip)?,
        Store::Edeka => scrapers::edeka::find_market(zip)?,
        Store::Lidl => {
            // Präsenzprüfung ist in beiden Quellen identisch (Store-Finder);
            // die passende wählen wir trotzdem konsistent zur Angebotsquelle.
            let found = if lidl_use_prospekt() {
                scrapers::lidl_prospekt::find_market(zip)?
            } else {
                scrapers::lidl::find_market(zip)?
            };
            match found {
                Some(m) => m,
                None => return Ok(None),
            }
        }
        Store::AldiNord => match scrapers::aldi_nord::find_market(zip)? {
            Some(m) => m,
            None => return Ok(None),
        },
        Store::AldiSued => match scrapers::aldi_sued::find_market(zip)? {
            Some(m) => m,
            None => return Ok(None),
        },
    };
    println!("Markt gefunden: {} (ID: {})", market.name, market.id);
    println!("Lade Angebote...");
    let offers = match store {
        Store::Rewe => scrapers::rewe::fetch_offers(&market, cert, key)?,
        Store::Penny => scrapers::penny::fetch_offers(&market)?,
        Store::Kaufland => scrapers::kaufland::fetch_offers(&market)?,
        Store::Lidl => {
            if lidl_use_prospekt() {
                scrapers::lidl_prospekt::fetch_offers(&market, zip, lidl_prospekt_max_pages())?
            } else {
                scrapers::lidl::fetch_offers(&market, zip)?
            }
        }
        Store::Netto => scrapers::netto::fetch_offers(&market)?,
        Store::AldiNord => scrapers::aldi_nord::fetch_offers(&market)?,
        Store::AldiSued => scrapers::aldi_sued::fetch_offers(&market)?,
        Store::Edeka => scrapers::edeka::fetch_offers(&market)?,
    };
    Ok(Some((market, offers)))
}

pub fn save_offers(db: &str, market: &Market, offers: &[Offer]) -> Result<()> {
    let conn = db::open(db)?;
    db::upsert_market(&conn, market)?;
    for offer in offers {
        db::upsert_offer(&conn, offer)?;
    }
    Ok(())
}
