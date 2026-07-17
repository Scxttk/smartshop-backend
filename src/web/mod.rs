//! Server-gerendertes Web-Dashboard (HTML, kein JavaScript).
//! Wird vom serve-Subcommand zusätzlich zur JSON-API (unter /api) gemountet.

mod html;

use std::sync::Arc;

use axum::extract::{Form, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
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
        .route("/compare", get(compare))
        .route("/watchlist", get(watchlist))
        .route("/watchlist/add", post(watchlist_add))
        .route("/watchlist/remove", post(watchlist_remove))
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

// ---------- /compare ----------

/// Gruppierung wie in api::get_compare / CLI-compare: nach normalisiertem
/// Produktnamen, Reihenfolge des Auftretens; innerhalb der Gruppe günstigster
/// zuerst; gleicher Markt + gleicher Preis nur einmal.
fn compare_groups(offers: &[Offer]) -> Vec<(String, Vec<&Offer>)> {
    let mut groups: Vec<(String, Vec<&Offer>)> = Vec::new();
    for offer in offers {
        let key = units::normalize_name(&display_name(offer));
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(offer),
            None => groups.push((key, vec![offer])),
        }
    }
    let mut result = Vec::new();
    for (_, mut group) in groups {
        group.sort_by(|a, b| {
            a.price
                .unwrap_or(f64::INFINITY)
                .total_cmp(&b.price.unwrap_or(f64::INFINITY))
        });
        let name = display_name(group[0]);
        let mut printed = std::collections::HashSet::new();
        let deduped: Vec<&Offer> = group
            .into_iter()
            .filter(|o| printed.insert((o.market_id.clone(), o.price.map(|p| (p * 100.0) as i64))))
            .collect();
        result.push((name, deduped));
    }
    result
}

async fn compare(
    State(state): SharedState,
    Query(params): Query<QueryParam>,
) -> Result<Html<String>, WebError> {
    let q = params.q.unwrap_or_default();
    let mut body = search_form("/compare", "q", &q, "Produkt");
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
            for (name, group) in compare_groups(&offers) {
                body.push_str(&format!("<h2>{}</h2>", escape(&name)));
                let rows: Vec<Vec<String>> = group
                    .iter()
                    .map(|o| {
                        vec![
                            escape(names.get(&o.market_id).unwrap_or(&o.market_id)),
                            price(o.price),
                            unit_price_str(o).map(|u| escape(&u)).unwrap_or_else(|| "–".into()),
                            o.subtitle.as_deref().map(escape).unwrap_or_default(),
                        ]
                    })
                    .collect();
                body.push_str(&table(&["Markt", ">Preis", ">Grundpreis", "Details"], &rows));
            }
        }
    }
    Ok(Html(page("Vergleich", &body)))
}

// ---------- /watchlist ----------

async fn watchlist(State(state): SharedState) -> Result<Html<String>, WebError> {
    let conn = db::open(&state.db_path)?;
    let watches = db::watches(&conn)?;
    let names = market_names(&conn)?;

    let mut body = String::from(
        "<h2>Neue Beobachtung</h2>\
<form method=\"post\" action=\"/watchlist/add\">\
<label>Suchbegriff <input type=\"text\" name=\"query\" required></label> \
<label>Max-Preis (€) <input type=\"number\" name=\"max_price\" step=\"0.01\" min=\"0\"></label> \
<button type=\"submit\">Anlegen</button></form>",
    );

    if watches.is_empty() {
        body.push_str("<p class=\"hinweis\">Keine Beobachtungen angelegt.</p>");
    } else {
        body.push_str("<h2>Beobachtungen</h2>");
        let rows: Vec<Vec<String>> = watches
            .iter()
            .map(|w| {
                let hits = db::watch_hits(&conn, w).map(|h| h.len()).unwrap_or(0);
                vec![
                    format!("#{}", w.id),
                    format!(
                        "<a href=\"/search?q={}\">{}</a>",
                        urlencode(&w.query),
                        escape(&w.query)
                    ),
                    w.max_price.map(|p| format!("bis {p:.2} €")).unwrap_or_else(|| "–".into()),
                    format!("{hits}"),
                    escape(&w.created_at),
                    format!(
                        "<form class=\"inline\" method=\"post\" action=\"/watchlist/remove\">\
<input type=\"hidden\" name=\"id\" value=\"{}\">\
<button type=\"submit\">Entfernen</button></form>",
                        w.id
                    ),
                ]
            })
            .collect();
        body.push_str(&table(
            &["ID", "Suchbegriff", ">Max-Preis", ">Treffer", "Angelegt am", ""],
            &rows,
        ));

        // Aktuelle Treffer wie bei `watch check`
        for w in &watches {
            let hits = db::watch_hits(&conn, w)?;
            if hits.is_empty() {
                continue;
            }
            body.push_str(&format!("<h2>Treffer für '{}'</h2>", escape(&w.query)));
            let rows: Vec<Vec<String>> = hits
                .iter()
                .map(|o| {
                    vec![
                        escape(names.get(&o.market_id).unwrap_or(&o.market_id)),
                        escape(&display_name(o)),
                        price(o.price),
                    ]
                })
                .collect();
            body.push_str(&table(&["Markt", "Produkt", ">Preis"], &rows));
        }
    }
    Ok(Html(page("Watchlist", &body)))
}

#[derive(Deserialize)]
struct AddWatchForm {
    query: String,
    max_price: Option<String>,
}

async fn watchlist_add(
    State(state): SharedState,
    Form(form): Form<AddWatchForm>,
) -> Result<Redirect, WebError> {
    let query = form.query.trim();
    if query.is_empty() {
        return Err(WebError::BadRequest("Feld 'query' fehlt oder ist leer.".to_string()));
    }
    // Leeres Zahlenfeld kommt als "" an, daher manuell parsen
    let max_price = match form.max_price.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(s) => Some(s.replace(',', ".").parse::<f64>().map_err(|_| {
            WebError::BadRequest("Feld 'max_price' muss eine Zahl sein.".to_string())
        })?),
    };
    let conn = db::open(&state.db_path)?;
    db::add_watch(&conn, query, max_price)?;
    Ok(Redirect::to("/watchlist"))
}

#[derive(Deserialize)]
struct RemoveWatchForm {
    id: i64,
}

async fn watchlist_remove(
    State(state): SharedState,
    Form(form): Form<RemoveWatchForm>,
) -> Result<Redirect, WebError> {
    let conn = db::open(&state.db_path)?;
    db::remove_watch(&conn, form.id)?;
    Ok(Redirect::to("/watchlist"))
}
