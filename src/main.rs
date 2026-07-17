mod db;
mod models;
mod scrapers;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand, ValueEnum};

use crate::models::Offer;

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
}

#[derive(Subcommand)]
enum Command {
    /// Angebote eines Markts abrufen und speichern
    Fetch {
        /// Postleitzahl des Markts
        #[arg(long)]
        zip: String,

        /// Supermarkt
        #[arg(long, value_enum, default_value_t = Store::Rewe)]
        store: Store,

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
        Command::Fetch { zip, store, cert, key, dry_run, db } => {
            fetch(zip, store, cert, key, dry_run, db)
        }
        Command::Search { query, max_price, db } => search(query, max_price, db),
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

fn fetch(zip: String, store: Store, cert: String, key: String, dry_run: bool, db: String) -> Result<()> {
    let (market, offers) = match store {
        Store::Rewe => {
            println!("Suche Rewe-Markt für PLZ {zip}...");
            let market = scrapers::rewe::find_market(&zip, &cert, &key)?;
            println!("Markt gefunden: {} (ID: {})", market.name, market.id);
            println!("Lade Angebote...");
            let offers = scrapers::rewe::fetch_offers(&market, &cert, &key)?;
            (market, offers)
        }
        Store::Penny => {
            // Wird vom Penny-Scraper-Branch (feature/penny-scraper) angebunden.
            bail!("Penny-Scraper ist noch nicht verfügbar.");
        }
    };
    println!("{} Angebote gefunden.", offers.len());

    if dry_run {
        for offer in &offers {
            println!("  {}", format_offer(offer));
        }
    } else {
        let conn = db::open(&db)?;
        db::upsert_market(&conn, &market)?;
        for offer in &offers {
            db::upsert_offer(&conn, offer)?;
        }
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
