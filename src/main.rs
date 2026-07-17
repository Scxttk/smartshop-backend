use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

use smartshop::models::Offer;
use smartshop::{db, scrapers, units};

#[derive(Parser)]
#[command(name = "smartshop", about = "Supermarkt-Angebote scrapen und speichern")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum Store {
    Rewe,
    Penny,
    Kaufland,
    Lidl,
}

#[derive(Subcommand)]
enum Command {
    /// Angebote eines Markts abrufen und speichern
    Fetch {
        /// Postleitzahl des Markts
        #[arg(long)]
        zip: String,

        /// Supermarkt
        #[arg(long, value_enum, default_value_t = Store::Rewe, conflicts_with = "all_stores")]
        store: Store,

        /// Alle Supermärkte nacheinander abrufen
        #[arg(long, default_value_t = false)]
        all_stores: bool,

        /// Pfad zum Rewe TLS-Zertifikat (PEM)
        #[arg(long, default_value = "cert.pem")]
        cert: String,

        /// Pfad zum privaten Schlüssel
        #[arg(long, default_value = "private.key")]
        key: String,

        /// Nur ausgeben, nicht speichern
        #[arg(long, default_value_t = false)]
        dry_run: bool,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Gespeicherte Angebote nach Titel durchsuchen
    Search {
        /// Suchbegriff (Teilstring des Titels)
        query: String,

        /// Nur Angebote bis zu diesem Preis
        #[arg(long)]
        max_price: Option<f64>,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Preise eines Produkts über alle gespeicherten Märkte vergleichen
    Compare {
        /// Suchbegriff (Teilstring des Titels)
        query: String,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
    /// Preisverlauf eines Produkts anzeigen
    History {
        /// Suchbegriff (Teilstring des Titels)
        query: String,

        /// Pfad zur SQLite-Datenbank
        #[arg(long, default_value = "smartshop.db")]
        db: String,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Fetch { zip, store, all_stores, cert, key, dry_run, db } => {
            if all_stores {
                fetch_all(zip, cert, key, dry_run, db)
            } else {
                fetch(zip, store, cert, key, dry_run, db)
            }
        }
        Command::Search { query, max_price, db } => search(query, max_price, db),
        Command::Compare { query, db } => compare(query, db),
        Command::History { query, db } => history(query, db),
    }
}

fn history(query: String, db: String) -> Result<()> {
    let conn = db::open(&db)?;
    let points = db::price_history(&conn, &query)?;
    if points.is_empty() {
        println!("Kein Preisverlauf für '{query}' gefunden.");
        return Ok(());
    }
    let mut current_title = String::new();
    for p in &points {
        if p.title != current_title {
            println!("{} ({})", p.title, p.market_id);
            current_title = p.title.clone();
        }
        let price = p.price.map(|v| format!("{v:.2} €")).unwrap_or_else(|| "-".to_string());
        println!("  {}  {}", p.seen_at, price);
    }
    Ok(())
}

impl Store {
    const ALL: [Store; 4] = [Store::Rewe, Store::Penny, Store::Kaufland, Store::Lidl];

    fn label(&self) -> &'static str {
        match self {
            Store::Rewe => "Rewe",
            Store::Penny => "Penny",
            Store::Kaufland => "Kaufland",
            Store::Lidl => "Lidl",
        }
    }
}

fn scrape_store(store: Store, zip: &str, cert: &str, key: &str) -> Result<(smartshop::models::Market, Vec<Offer>)> {
    println!("Suche {}-Markt für PLZ {zip}...", store.label());
    let market = match store {
        Store::Rewe => scrapers::rewe::find_market(zip, cert, key)?,
        Store::Penny => scrapers::penny::find_market(zip)?,
        Store::Kaufland => scrapers::kaufland::find_market(zip)?,
        Store::Lidl => scrapers::lidl::find_market(zip)?,
    };
    println!("Markt gefunden: {} (ID: {})", market.name, market.id);
    println!("Lade Angebote...");
    let offers = match store {
        Store::Rewe => scrapers::rewe::fetch_offers(&market, cert, key)?,
        Store::Penny => scrapers::penny::fetch_offers(&market)?,
        Store::Kaufland => scrapers::kaufland::fetch_offers(&market)?,
        Store::Lidl => scrapers::lidl::fetch_offers(&market)?,
    };
    Ok((market, offers))
}

fn fetch_all(zip: String, cert: String, key: String, dry_run: bool, db: String) -> Result<()> {
    struct Row {
        store: &'static str,
        market: String,
        result: std::result::Result<usize, String>,
    }
    let mut rows = Vec::new();

    for store in Store::ALL {
        match scrape_store(store, &zip, &cert, &key) {
            Ok((market, offers)) => {
                let count = offers.len();
                println!("{} Angebote gefunden.", count);
                if dry_run {
                    for offer in &offers {
                        println!("  {}", format_offer(offer));
                    }
                    rows.push(Row { store: store.label(), market: market.name, result: Ok(count) });
                } else {
                    match save_offers(&db, &market, &offers) {
                        Ok(()) => {
                            println!("{count} Angebote in '{db}' gespeichert.");
                            rows.push(Row { store: store.label(), market: market.name, result: Ok(count) });
                        }
                        Err(e) => {
                            eprintln!("{}: Speichern fehlgeschlagen: {e:#}", store.label());
                            rows.push(Row { store: store.label(), market: market.name, result: Err(format!("{e:#}")) });
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("{}: Fehler: {e:#}", store.label());
                rows.push(Row { store: store.label(), market: "-".to_string(), result: Err(format!("{e:#}")) });
            }
        }
        println!();
    }

    println!("Zusammenfassung:");
    println!("  {:<10} {:<28} {}", "Markt", "Filiale", "Ergebnis");
    for row in &rows {
        let result = match &row.result {
            Ok(n) => format!("{n} Angebote"),
            Err(e) => {
                let first = e.lines().next().unwrap_or("Fehler");
                format!("FEHLER: {first}")
            }
        };
        println!("  {:<10} {:<28} {}", row.store, row.market, result);
    }
    Ok(())
}

fn save_offers(db: &str, market: &smartshop::models::Market, offers: &[Offer]) -> Result<()> {
    let conn = db::open(db)?;
    db::upsert_market(&conn, market)?;
    for offer in offers {
        db::upsert_offer(&conn, offer)?;
    }
    Ok(())
}

fn fetch(zip: String, store: Store, cert: String, key: String, dry_run: bool, db: String) -> Result<()> {
    let (market, offers) = scrape_store(store, &zip, &cert, &key)?;
    println!("{} Angebote gefunden.", offers.len());

    if dry_run {
        for offer in &offers {
            println!("  {}", format_offer(offer));
        }
    } else {
        save_offers(&db, &market, &offers)?;
        println!("{} Angebote in '{}' gespeichert.", offers.len(), db);
    }

    Ok(())
}

fn search(query: String, max_price: Option<f64>, db: String) -> Result<()> {
    let conn = db::open(&db)?;
    let offers = db::search_offers(&conn, &query, max_price)?;
    if offers.is_empty() {
        println!("Keine Angebote für '{query}' gefunden.");
    } else {
        for offer in &offers {
            println!("  {}", format_offer(offer));
        }
        println!("{} Treffer.", offers.len());
    }
    Ok(())
}

// Anzeigename: Titel, plus Untertitel wenn er kein reiner Mengen-Text ist
// (Kaufland: Marke im Titel, Produktname im Untertitel).
fn display_name(offer: &Offer) -> String {
    match &offer.subtitle {
        Some(sub) if units::parse_quantity(sub).is_none() && !sub.is_empty() => {
            format!("{} {}", offer.title, sub)
        }
        _ => offer.title.clone(),
    }
}

fn compare(query: String, db: String) -> Result<()> {
    let conn = db::open(&db)?;
    let offers = db::search_offers_broad(&conn, &query)?;
    if offers.is_empty() {
        println!("Keine Angebote für '{query}' gefunden.");
        return Ok(());
    }
    let market_names: std::collections::HashMap<String, String> = db::markets(&conn)?
        .into_iter()
        .map(|m| (m.id, m.name))
        .collect();

    // Nach normalisiertem Produktnamen gruppieren, Reihenfolge des Auftretens
    let mut groups: Vec<(String, Vec<&Offer>)> = Vec::new();
    for offer in &offers {
        let key = units::normalize_name(&display_name(offer));
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(offer),
            None => groups.push((key, vec![offer])),
        }
    }

    for (_, mut group) in groups {
        // Günstigster Markt zuerst; Angebote ohne Preis ans Ende
        group.sort_by(|a, b| {
            a.price
                .unwrap_or(f64::INFINITY)
                .total_cmp(&b.price.unwrap_or(f64::INFINITY))
        });
        println!("{}", display_name(group[0]));
        let mut printed = std::collections::HashSet::new();
        for offer in group {
            // Gleicher Markt + gleicher Preis (z. B. aktuelle und nächste Woche) nur einmal
            if !printed.insert((offer.market_id.clone(), offer.price.map(|p| (p * 100.0) as i64))) {
                continue;
            }
            let market = market_names
                .get(&offer.market_id)
                .map(String::as_str)
                .unwrap_or(offer.market_id.as_str());
            let price = offer
                .price
                .map(|p| format!("{p:.2} €"))
                .unwrap_or_else(|| "-".to_string());
            let unit = units::derive_unit_price(
                offer.price,
                &[offer.subtitle.as_deref(), offer.overline.as_deref(), Some(&offer.title)],
            )
            .map(|up| format!("  ({})", up.format()))
            .unwrap_or_default();
            let sub = offer.subtitle.as_ref().map(|s| format!(" ({s})")).unwrap_or_default();
            println!("  {market:<20} {price:>8}{unit}{sub}");
        }
    }
    Ok(())
}

fn format_offer(offer: &Offer) -> String {
    let price_str = offer.price
        .map(|p| format!("{p:.2} €"))
        .unwrap_or_else(|| "-".to_string());
    let reg_str = offer.regular_price
        .map(|p| format!(" (statt {p:.2} €)"))
        .unwrap_or_default();
    let dates = offer.valid_from.as_ref()
        .map(|f| format!("  [{} – {}]", f, offer.valid_until.as_deref().unwrap_or("?")))
        .unwrap_or_default();
    format!(
        "[{cat}] {title}{sub} — {price_str}{reg_str}{dates}",
        cat = offer.category.as_deref().unwrap_or("?"),
        title = offer.title,
        sub = offer.subtitle.as_ref().map(|s| format!(" ({s})")).unwrap_or_default(),
    )
}
