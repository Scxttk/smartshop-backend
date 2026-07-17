use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::models::{Market, Offer};
use crate::{db, units};

// Hinweis zur Nebenläufigkeit: rusqlite::Connection ist nicht Sync und kann
// daher nicht als geteilter axum-State verwendet werden. Stattdessen hält der
// State nur den DB-Pfad und jeder Handler öffnet pro Request eine eigene
// Verbindung (db::open) — bei SQLite im WAL-Modus günstig und für parallele
// Lesezugriffe unproblematisch.
pub struct AppState {
    pub db_path: String,
}

type SharedState = State<Arc<AppState>>;

/// Fehler der API: 400 bei fehlenden/ungültigen Parametern, 500 bei
/// DB-/internen Fehlern — immer mit JSON-Body {"error": "..."}.
enum ApiError {
    BadRequest(String),
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Internal(e) => {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Interner Fehler: {e:#}"))
            }
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::Internal(e)
    }
}

fn missing_q() -> ApiError {
    ApiError::BadRequest("Parameter 'q' fehlt oder ist leer.".to_string())
}

pub fn router(db_path: String) -> Router {
    Router::new()
        .route("/markets", get(get_markets))
        .route("/offers", get(get_offers))
        .route("/compare", get(get_compare))
        .route("/stats", get(get_stats))
        .route("/history", get(get_history))
        .route("/deals", get(get_deals))
        .route("/watches", get(get_watches))
        .route("/watches/check", get(get_watches_check))
        .route("/list", get(get_list))
        .route("/list/suggest", get(get_list_suggest))
        .with_state(Arc::new(AppState { db_path }))
}

/// HTTP-Server starten (blockiert bis zum Abbruch).
pub fn serve(port: u16, db_path: String) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
        println!(
            "Web-UI läuft auf http://0.0.0.0:{port}, JSON-API auf http://0.0.0.0:{port}/api (DB: {db_path})"
        );
        let app = crate::web::router(db_path.clone()).nest("/api", router(db_path));
        axum::serve(listener, app).await?;
        Ok(())
    })
}

// Anzeigename wie im CLI (main.rs): Titel plus Untertitel, wenn dieser kein
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

fn market_names(conn: &rusqlite::Connection) -> Result<HashMap<String, String>> {
    Ok(db::markets(conn)?.into_iter().map(|m| (m.id, m.name)).collect())
}

async fn get_markets(State(state): SharedState) -> Result<Json<Vec<Market>>, ApiError> {
    let conn = db::open(&state.db_path)?;
    Ok(Json(db::markets(&conn)?))
}

#[derive(Deserialize)]
struct OffersParams {
    q: Option<String>,
    max_price: Option<f64>,
    market: Option<String>,
}

async fn get_offers(
    State(state): SharedState,
    Query(params): Query<OffersParams>,
) -> Result<Json<Vec<Offer>>, ApiError> {
    let q = params.q.filter(|q| !q.is_empty()).ok_or_else(missing_q)?;
    let conn = db::open(&state.db_path)?;
    let mut offers = db::search_offers_broad(&conn, &q)?;
    if let Some(max) = params.max_price {
        offers.retain(|o| o.price.is_some_and(|p| p <= max));
    }
    if let Some(market) = params.market {
        offers.retain(|o| o.market_id == market);
    }
    Ok(Json(offers))
}

#[derive(Deserialize)]
struct QueryParam {
    q: Option<String>,
}

#[derive(Serialize)]
struct CompareOffer {
    market_id: String,
    market: String,
    price: Option<f64>,
    unit_price: Option<String>,
    subtitle: Option<String>,
}

#[derive(Serialize)]
struct CompareGroup {
    name: String,
    offers: Vec<CompareOffer>,
}

async fn get_compare(
    State(state): SharedState,
    Query(params): Query<QueryParam>,
) -> Result<Json<Vec<CompareGroup>>, ApiError> {
    let q = params.q.filter(|q| !q.is_empty()).ok_or_else(missing_q)?;
    let conn = db::open(&state.db_path)?;
    let offers = db::search_offers_broad(&conn, &q)?;
    let names = market_names(&conn)?;

    // Gruppierung wie beim CLI-compare: nach normalisiertem Produktnamen,
    // Reihenfolge des Auftretens; innerhalb der Gruppe günstigster zuerst.
    let mut groups: Vec<(String, Vec<&Offer>)> = Vec::new();
    for offer in &offers {
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
        let mut printed = HashSet::new();
        let mut entries = Vec::new();
        for offer in group {
            // Gleicher Markt + gleicher Preis (z. B. aktuelle und nächste Woche) nur einmal
            if !printed.insert((offer.market_id.clone(), offer.price.map(|p| (p * 100.0) as i64))) {
                continue;
            }
            entries.push(CompareOffer {
                market: names
                    .get(&offer.market_id)
                    .cloned()
                    .unwrap_or_else(|| offer.market_id.clone()),
                market_id: offer.market_id.clone(),
                price: offer.price,
                unit_price: unit_price_str(offer),
                subtitle: offer.subtitle.clone(),
            });
        }
        result.push(CompareGroup { name, offers: entries });
    }
    Ok(Json(result))
}

#[derive(Serialize)]
struct MarketStatsDto {
    market_id: String,
    market_name: String,
    offer_count: i64,
    valid_from_min: Option<String>,
    valid_until_max: Option<String>,
    avg_discount_pct: Option<f64>,
}

#[derive(Serialize)]
struct DiscountDto {
    market_name: String,
    title: String,
    subtitle: Option<String>,
    price: f64,
    regular_price: f64,
    discount_pct: f64,
}

#[derive(Serialize)]
struct StatsResponse {
    markets: Vec<MarketStatsDto>,
    top_discounts: Vec<DiscountDto>,
}

async fn get_stats(State(state): SharedState) -> Result<Json<StatsResponse>, ApiError> {
    let conn = db::open(&state.db_path)?;
    let markets = db::market_stats(&conn)?
        .into_iter()
        .map(|s| MarketStatsDto {
            market_id: s.market_id,
            market_name: s.market_name,
            offer_count: s.offer_count,
            valid_from_min: s.valid_from_min,
            valid_until_max: s.valid_until_max,
            avg_discount_pct: s.avg_discount_pct,
        })
        .collect();
    let top_discounts = db::top_discounts(&conn, 10)?
        .into_iter()
        .map(|d| DiscountDto {
            market_name: d.market_name,
            title: d.title,
            subtitle: d.subtitle,
            price: d.price,
            regular_price: d.regular_price,
            discount_pct: d.discount_pct,
        })
        .collect();
    Ok(Json(StatsResponse { markets, top_discounts }))
}

#[derive(Serialize)]
struct PricePointDto {
    title: String,
    market_id: String,
    price: Option<f64>,
    seen_at: String,
}

async fn get_history(
    State(state): SharedState,
    Query(params): Query<QueryParam>,
) -> Result<Json<Vec<PricePointDto>>, ApiError> {
    let q = params.q.filter(|q| !q.is_empty()).ok_or_else(missing_q)?;
    let conn = db::open(&state.db_path)?;
    let points = db::price_history(&conn, &q)?
        .into_iter()
        .map(|p| PricePointDto {
            title: p.title,
            market_id: p.market_id,
            price: p.price,
            seen_at: p.seen_at,
        })
        .collect();
    Ok(Json(points))
}

#[derive(Deserialize)]
struct DealsParams {
    since: Option<String>,
}

#[derive(Serialize)]
struct PriceDropDto {
    title: String,
    market_id: String,
    old_price: f64,
    new_price: f64,
    old_seen_at: String,
    new_seen_at: String,
}

async fn get_deals(
    State(state): SharedState,
    Query(params): Query<DealsParams>,
) -> Result<Json<Vec<PriceDropDto>>, ApiError> {
    // since manuell parsen, um bei "?since=abc" gezielt 400 zu liefern
    let since = match params.since {
        Some(s) => Some(s.parse::<i64>().map_err(|_| {
            ApiError::BadRequest("Parameter 'since' muss eine ganze Zahl (Tage) sein.".to_string())
        })?),
        None => None,
    };
    let conn = db::open(&state.db_path)?;
    let drops = db::price_drops(&conn, since)?
        .into_iter()
        .map(|d| PriceDropDto {
            title: d.title,
            market_id: d.market_id,
            old_price: d.old_price,
            new_price: d.new_price,
            old_seen_at: d.old_seen_at,
            new_seen_at: d.new_seen_at,
        })
        .collect();
    Ok(Json(drops))
}

#[derive(Serialize)]
struct WatchDto {
    id: i64,
    query: String,
    max_price: Option<f64>,
    created_at: String,
}

async fn get_watches(State(state): SharedState) -> Result<Json<Vec<WatchDto>>, ApiError> {
    let conn = db::open(&state.db_path)?;
    let watches = db::watches(&conn)?
        .into_iter()
        .map(|w| WatchDto {
            id: w.id,
            query: w.query,
            max_price: w.max_price,
            created_at: w.created_at,
        })
        .collect();
    Ok(Json(watches))
}

#[derive(Serialize)]
struct WatchCheckEntry {
    id: i64,
    query: String,
    max_price: Option<f64>,
    hits: Vec<Offer>,
}

#[derive(Serialize)]
struct WatchCheckResponse {
    hits: bool,
    watches: Vec<WatchCheckEntry>,
}

async fn get_watches_check(
    State(state): SharedState,
) -> Result<Json<WatchCheckResponse>, ApiError> {
    let conn = db::open(&state.db_path)?;
    let mut any = false;
    let mut entries = Vec::new();
    for w in db::watches(&conn)? {
        let hits = db::watch_hits(&conn, &w)?;
        any = any || !hits.is_empty();
        entries.push(WatchCheckEntry {
            id: w.id,
            query: w.query,
            max_price: w.max_price,
            hits,
        });
    }
    Ok(Json(WatchCheckResponse { hits: any, watches: entries }))
}

#[derive(Serialize)]
struct ListItemDto {
    id: i64,
    item: String,
    added_at: String,
}

async fn get_list(State(state): SharedState) -> Result<Json<Vec<ListItemDto>>, ApiError> {
    let conn = db::open(&state.db_path)?;
    let items = db::list_items(&conn)?
        .into_iter()
        .map(|it| ListItemDto { id: it.id, item: it.item, added_at: it.added_at })
        .collect();
    Ok(Json(items))
}

#[derive(Serialize)]
struct SuggestOffer {
    title: String,
    market_id: String,
    market: String,
    price: f64,
    unit_price: Option<String>,
}

#[derive(Serialize)]
struct SuggestEntry {
    item: String,
    offer: Option<SuggestOffer>,
}

async fn get_list_suggest(
    State(state): SharedState,
) -> Result<Json<Vec<SuggestEntry>>, ApiError> {
    let conn = db::open(&state.db_path)?;
    let names = market_names(&conn)?;
    let mut result = Vec::new();
    // Wie beim CLI-suggest: pro Listen-Artikel das günstigste Angebot mit
    // Preis über alle Märkte (search_offers_broad sortiert bereits nach Preis).
    for it in db::list_items(&conn)? {
        let mut offers = db::search_offers_broad(&conn, &it.item)?;
        offers.retain(|o| o.price.is_some());
        let offer = offers.first().map(|best| SuggestOffer {
            title: display_name(best),
            market: names
                .get(&best.market_id)
                .cloned()
                .unwrap_or_else(|| best.market_id.clone()),
            market_id: best.market_id.clone(),
            price: best.price.unwrap(),
            unit_price: unit_price_str(best),
        });
        result.push(SuggestEntry { item: it.item, offer });
    }
    Ok(Json(result))
}
