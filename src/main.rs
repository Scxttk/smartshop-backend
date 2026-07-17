mod db;
mod models;
mod scrapers;

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "smartshop", about = "Supermarkt-Angebote scrapen und speichern")]
struct Cli {
    /// Postleitzahl des Markts
    #[arg(long)]
    zip: String,

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    println!("Suche Rewe-Markt für PLZ {}...", cli.zip);
    let market = scrapers::rewe::find_market(&cli.zip, &cli.cert, &cli.key)?;
    println!("Markt gefunden: {} (ID: {})", market.name, market.id);

    println!("Lade Angebote...");
    let offers = scrapers::rewe::fetch_offers(&market, &cli.cert, &cli.key)?;
    println!("{} Angebote gefunden.", offers.len());

    if cli.dry_run {
        for offer in &offers {
            let price_str = offer.price
                .map(|p| format!("{:.2} €", p))
                .unwrap_or_else(|| "-".to_string());
            let reg_str = offer.regular_price
                .map(|p| format!(" (statt {:.2} €)", p))
                .unwrap_or_default();
            let dates = offer.valid_from.as_ref()
                .map(|f| format!("  [{} – {}]", f, offer.valid_until.as_deref().unwrap_or("?")))
                .unwrap_or_default();
            println!(
                "  [{cat}] {title}{sub} — {price}{reg}{dates}",
                cat = offer.category.as_deref().unwrap_or("?"),
                title = offer.title,
                sub = offer.subtitle.as_ref().map(|s| format!(" ({})", s)).unwrap_or_default(),
                price = price_str,
                reg = reg_str,
            );
        }
    } else {
        let conn = db::open(&cli.db)?;
        db::upsert_market(&conn, &market)?;
        for offer in &offers {
            db::upsert_offer(&conn, offer)?;
        }
        println!("{} Angebote in '{}' gespeichert.", offers.len(), cli.db);
    }

    Ok(())
}
