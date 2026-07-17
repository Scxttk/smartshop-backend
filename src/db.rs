use anyhow::Result;
use rusqlite::{Connection, params};

use crate::models::{Market, Offer};

pub fn open(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    init_schema(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch("
        CREATE TABLE IF NOT EXISTS markets (
            id   TEXT PRIMARY KEY,
            name TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS offers (
            id            TEXT PRIMARY KEY,
            market_id     TEXT NOT NULL,
            title         TEXT NOT NULL,
            subtitle      TEXT,
            overline      TEXT,
            price         REAL,
            regular_price REAL,
            category      TEXT,
            nutri_score   TEXT,
            valid_from    TEXT,
            valid_until   TEXT,
            images        TEXT,
            biozid        INTEGER DEFAULT 0,
            flyer_page    INTEGER,
            fetched_at    TEXT DEFAULT (datetime('now')),
            FOREIGN KEY (market_id) REFERENCES markets(id)
        );
    ")?;
    Ok(())
}

pub fn upsert_market(conn: &Connection, market: &Market) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO markets (id, name) VALUES (?1, ?2)",
        params![market.id, market.name],
    )?;
    Ok(())
}

pub fn search_offers(conn: &Connection, query: &str, max_price: Option<f64>) -> Result<Vec<Offer>> {
    let mut stmt = conn.prepare(
        "SELECT id, market_id, title, subtitle, overline, price, regular_price,
                category, nutri_score, valid_from, valid_until, images, biozid, flyer_page
         FROM offers
         WHERE title LIKE '%' || ?1 || '%'
           AND (?2 IS NULL OR price <= ?2)
         ORDER BY price ASC",
    )?;
    let rows = stmt.query_map(params![query, max_price], |row| {
        let images_json: String = row.get(11)?;
        Ok(Offer {
            id: row.get(0)?,
            market_id: row.get(1)?,
            title: row.get(2)?,
            subtitle: row.get(3)?,
            overline: row.get(4)?,
            price: row.get(5)?,
            regular_price: row.get(6)?,
            category: row.get(7)?,
            nutri_score: row.get(8)?,
            valid_from: row.get(9)?,
            valid_until: row.get(10)?,
            images: serde_json::from_str(&images_json).unwrap_or_default(),
            biozid: row.get::<_, i64>(12)? != 0,
            flyer_page: row.get(13)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn upsert_offer(conn: &Connection, offer: &Offer) -> Result<()> {
    let images = serde_json::to_string(&offer.images)?;
    conn.execute(
        "INSERT OR REPLACE INTO offers
            (id, market_id, title, subtitle, overline, price, regular_price,
             category, nutri_score, valid_from, valid_until, images, biozid, flyer_page)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        params![
            offer.id,
            offer.market_id,
            offer.title,
            offer.subtitle,
            offer.overline,
            offer.price,
            offer.regular_price,
            offer.category,
            offer.nutri_score,
            offer.valid_from,
            offer.valid_until,
            images,
            offer.biozid as i64,
            offer.flyer_page,
        ],
    )?;
    Ok(())
}
