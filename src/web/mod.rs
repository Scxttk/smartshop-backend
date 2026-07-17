//! Server-gerendertes Web-Dashboard (HTML, kein JavaScript).
//! Wird vom serve-Subcommand zusätzlich zur JSON-API (unter /api) gemountet.

mod html;

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::db;
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
        .with_state(Arc::new(WebState { db_path }))
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
