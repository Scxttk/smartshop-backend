//! Server-gerendertes Web-Dashboard (HTML, kein JavaScript).
//! Wird vom serve-Subcommand zusätzlich zur JSON-API (unter /api) gemountet.

mod html;

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use crate::models::Offer;
use crate::{db, units};
use html::{escape, page, price, table};

// Wie in api.rs: rusqlite::Connection ist nicht Sync, daher hält der State
// nur den DB-Pfad und jeder Handler öffnet pro Request eine Verbindung.
struct WebState {
    db_path: String,
}

type SharedState = State<Arc<WebState>>;

enum WebError {
    #[allow(dead_code)] // wird von den Formular-Seiten (Watchlist) verwendet
    BadRequest(String),
    Internal(anyhow::Error),
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            WebError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            WebError::Internal(e) => {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Interner Fehler: {e:#}"))
            }
        };
        let body = page("Fehler", &format!("<p>{}</p>", escape(&message)));
        (status, Html(body)).into_response()
    }
}

impl From<anyhow::Error> for WebError {
    fn from(e: anyhow::Error) -> Self {
        WebError::Internal(e)
    }
}

pub fn router(db_path: String) -> Router {
    Router::new()
        .route("/", get(overview))
        .route("/search", get(search))
        .with_state(Arc::new(WebState { db_path }))
}

// Anzeigename wie im CLI/in der API: Titel plus Untertitel, wenn dieser kein
// reiner Mengen-Text ist (Kaufland: Marke im Titel, Produkt im Untertitel).
fn display_name(offer: &Offer) -> String {
    match &offer.subtitle {
        Some(sub) if units::parse_quantity(sub).is_none() && !sub.is_empty() => {
            format!("{} {}", offer.title, sub)
        }
        _ => offer.title.clone(),
    }
}

fn unit_price_str(offer: &Offer) -> Option<String> {
    units::derive_unit_price(
        offer.price,
        &[offer.subtitle.as_deref(), offer.overline.as_deref(), Some(&offer.title)],
    )
    .map(|up| up.format())
}

fn market_names(conn: &rusqlite::Connection) -> anyhow::Result<std::collections::HashMap<String, String>> {
    Ok(db::markets(conn)?.into_iter().map(|m| (m.id, m.name)).collect())
}

/// GET-Suchformular; `action` ist der Ziel-Pfad, `name` der Parametername.
fn search_form(action: &str, name: &str, value: &str, label: &str) -> String {
    format!(
        "<form method=\"get\" action=\"{action}\">\
<label>{label} <input type=\"text\" name=\"{name}\" value=\"{}\" required></label> \
<button type=\"submit\">Suchen</button></form>",
        escape(value)
    )
}

#[derive(Deserialize)]
struct QueryParam {
    q: Option<String>,
}

// ---------- / (Übersicht) ----------

async fn overview(State(state): SharedState) -> Result<Html<String>, WebError> {
    let conn = db::open(&state.db_path)?;
    let stats = db::market_stats(&conn)?;

    let mut body = String::new();
    if stats.is_empty() {
        body.push_str("<p class=\"hinweis\">Keine Angebote gespeichert.</p>");
    } else {
        body.push_str("<h2>Angebote pro Markt</h2>");
        let rows: Vec<Vec<String>> = stats
            .iter()
            .map(|s| {
                vec![
                    escape(&s.market_name),
                    format!("{}", s.offer_count),
                    format!(
                        "{} – {}",
                        escape(s.valid_from_min.as_deref().unwrap_or("?")),
                        escape(s.valid_until_max.as_deref().unwrap_or("?"))
                    ),
                    s.avg_discount_pct
                        .map(|d| format!("{d:.0} %"))
                        .unwrap_or_else(|| "–".to_string()),
                ]
            })
            .collect();
        body.push_str(&table(&["Filiale", ">Angebote", "Gültigkeit", ">Ø Rabatt"], &rows));

        let discounts = db::top_discounts(&conn, 10)?;
        if !discounts.is_empty() {
            body.push_str("<h2>Top-Rabatte</h2>");
            let rows: Vec<Vec<String>> = discounts
                .iter()
                .map(|d| {
                    let name = match &d.subtitle {
                        Some(sub) if !sub.is_empty() => format!("{} {}", d.title, sub),
                        _ => d.title.clone(),
                    };
                    vec![
                        format!("-{:.0} %", d.discount_pct),
                        escape(&name),
                        format!("{} statt {}", price(Some(d.price)), price(Some(d.regular_price))),
                        escape(&d.market_name),
                    ]
                })
                .collect();
            body.push_str(&table(&[">Rabatt", "Produkt", "Preis", "Filiale"], &rows));
        }
    }
    Ok(Html(page("Übersicht", &body)))
}

// ---------- /search ----------

async fn search(
    State(state): SharedState,
    Query(params): Query<QueryParam>,
) -> Result<Html<String>, WebError> {
    let q = params.q.unwrap_or_default();
    let mut body = search_form("/search", "q", &q, "Suchbegriff");
    if !q.is_empty() {
        let conn = db::open(&state.db_path)?;
        let offers = db::search_offers_broad(&conn, &q)?;
        if offers.is_empty() {
            body.push_str(&format!(
                "<p class=\"hinweis\">Keine Angebote für '{}' gefunden.</p>",
                escape(&q)
            ));
        } else {
            let names = market_names(&conn)?;
            body.push_str(&format!("<p>{} Treffer.</p>", offers.len()));
            let rows: Vec<Vec<String>> = offers
                .iter()
                .map(|o| {
                    vec![
                        escape(names.get(&o.market_id).unwrap_or(&o.market_id)),
                        format!(
                            "<a href=\"/history?offer={}\">{}</a>",
                            urlencode(&o.title),
                            escape(&display_name(o))
                        ),
                        price(o.price),
                        unit_price_str(o).map(|u| escape(&u)).unwrap_or_else(|| "–".into()),
                    ]
                })
                .collect();
            body.push_str(&table(&["Markt", "Produkt", ">Preis", ">Grundpreis"], &rows));
        }
    }
    Ok(Html(page("Suche", &body)))
}

/// Minimales Prozent-Encoding für Query-Parameter-Werte in Links.
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
