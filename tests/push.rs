//! Tests für den Supabase-Push: Mapping (Offer -> SupabaseRow) und die
//! HTTP-Schicht gegen einen handgerollten Mock-Server (kein Live-Netzwerk).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use smartshop::models::{Market, Offer};
use smartshop::push::{self, PushConfig, PushOptions, SupabaseRow, chain_for, dedupe_rows, map_offer};
use smartshop::{db, units};

fn offer(title: &str, price: Option<f64>) -> Offer {
    Offer {
        id: Offer::build_id("m1", title, Some("2026-07-13")),
        market_id: "m1".to_string(),
        title: title.to_string(),
        subtitle: None,
        overline: None,
        price,
        regular_price: None,
        category: Some("Molkerei".to_string()),
        nutri_score: None,
        valid_from: Some("2026-07-13".to_string()),
        valid_until: Some("2026-07-19".to_string()),
        images: vec![],
        biozid: false,
        flyer_page: None,
    }
}

// ---------------------------------------------------------------- Mapping

#[test]
fn map_basic_fields() {
    let mut o = offer("Gouda", Some(1.99));
    o.regular_price = Some(2.79);
    let row = map_offer(&o, "REWE", Some("01219")).unwrap();
    assert_eq!(row.market, "REWE");
    assert_eq!(row.product, "Gouda");
    assert_eq!(row.price, 1.99);
    assert_eq!(row.regular_price, Some(2.79));
    assert_eq!(row.unit, "Stück");
    // Rohkategorie "Molkerei" wird normalisiert, Emoji aus der Keyword-Tabelle
    assert_eq!(row.category.as_deref(), Some("Molkerei & Eier"));
    assert_eq!(row.emoji.as_deref(), Some("🧀"));
    assert_eq!(row.valid_from.as_deref(), Some("2026-07-13"));
    assert_eq!(row.valid_until.as_deref(), Some("2026-07-19"));
    assert_eq!(row.brand, None);
    assert_eq!(row.ean, None);
    assert_eq!(row.source, "smartshop-rust");
    assert_eq!(row.region.as_deref(), Some("01219"));
}

#[test]
fn map_takes_first_image_and_keeps_emoji_fallback() {
    // Mit Bild: erste nicht-leere URL landet in image_url, Emoji bleibt gesetzt.
    let mut o = offer("Gouda", Some(1.99));
    o.images = vec![
        "".to_string(),
        "https://cdn.example/gouda-450.jpg".to_string(),
        "https://cdn.example/gouda-900.jpg".to_string(),
    ];
    let row = map_offer(&o, "REWE", None).unwrap();
    assert_eq!(row.image_url.as_deref(), Some("https://cdn.example/gouda-450.jpg"));
    assert_eq!(row.emoji.as_deref(), Some("🧀"));

    // Ohne Bild: image_url None, Emoji trägt weiterhin die Anzeige.
    let row = map_offer(&offer("Gouda", Some(1.99)), "REWE", None).unwrap();
    assert_eq!(row.image_url, None);
    assert_eq!(row.emoji.as_deref(), Some("🧀"));
}

#[test]
fn map_skips_offers_without_price() {
    assert!(map_offer(&offer("Gouda", None), "REWE", None).is_none());
}

#[test]
fn map_appends_informative_subtitle() {
    // Kaufland-Stil: Marke im Titel, Produkt im Untertitel
    let mut o = offer("K-Classic", Some(0.99));
    o.subtitle = Some("H-Milch 3,5%".to_string());
    let row = map_offer(&o, "Kaufland", None).unwrap();
    assert_eq!(row.product, "K-Classic H-Milch 3,5%");
}

#[test]
fn map_drops_pure_quantity_subtitle() {
    let mut o = offer("Gouda", Some(1.99));
    o.subtitle = Some("je 250-g-Packg.".to_string());
    let row = map_offer(&o, "REWE", None).unwrap();
    assert_eq!(row.product, "Gouda");
    // …aber die Menge landet im unit-Feld statt "Stück"
    assert_eq!(row.unit, "je 250-g-Packg.");
}

#[test]
fn map_puts_multipack_quantity_into_unit() {
    let mut o = offer("MILPRIMA Haltbare fettarme Milch", Some(7.8));
    o.subtitle = Some("je 12 x 1 l".to_string());
    let row = map_offer(&o, "Penny", None).unwrap();
    assert_eq!(row.unit, "je 12 x 1 l");
    assert_eq!(row.base_unit.as_deref(), Some("l"));
    assert_eq!(row.base_price, Some(0.65));
}

#[test]
fn map_keeps_subtitle_with_product_name_and_quantity() {
    // Kaufland-Stil: Untertitel trägt Produktname UND Menge — muss erhalten
    // bleiben, sonst kollabieren alle Angebote einer Marke beim Dedupe
    let mut o = offer("K-Classic", Some(0.99));
    o.subtitle = Some("Rispentomaten, 500-g-Schale".to_string());
    let row = map_offer(&o, "Kaufland", None).unwrap();
    assert_eq!(row.product, "K-Classic Rispentomaten, 500-g-Schale");
}

#[test]
fn map_computes_base_price_from_quantity() {
    let mut o = offer("Wein", Some(3.29));
    o.subtitle = Some("0.75 l".to_string());
    let row = map_offer(&o, "REWE", None).unwrap();
    assert_eq!(row.base_unit.as_deref(), Some("l"));
    // 3.29 / 0.75 = 4.386..., auf Cent gerundet
    assert_eq!(row.base_price, Some(4.39));
}

#[test]
fn map_prefers_explicit_base_price() {
    let mut o = offer("Butterkäse", Some(1.79));
    o.overline = Some("je 650-g-Packg. (1 kg = 2.76)".to_string());
    let row = map_offer(&o, "Penny", None).unwrap();
    assert_eq!(row.base_price, Some(2.76));
    assert_eq!(row.base_unit.as_deref(), Some("kg"));
}

#[test]
fn map_serializes_to_expected_json() {
    let mut o = offer("Gouda", Some(1.99));
    o.images = vec!["https://cdn.example/gouda-450.jpg".to_string()];
    let row = map_offer(&o, "REWE", Some("01219")).unwrap();
    let v = serde_json::to_value(&row).unwrap();
    assert_eq!(v["market"], "REWE");
    assert_eq!(v["price"], 1.99);
    assert_eq!(v["emoji"], "🧀");
    assert_eq!(v["image_url"], "https://cdn.example/gouda-450.jpg");
    assert_eq!(v["source"], "smartshop-rust");
    assert_eq!(v["region"], "01219");
}

#[test]
fn dedupe_on_conflict_key() {
    let a = map_offer(&offer("Gouda", Some(1.99)), "REWE", Some("01219")).unwrap();
    let b = map_offer(&offer("Gouda", Some(2.49)), "REWE", Some("01219")).unwrap();
    let mut c = a.clone();
    c.region = Some("10115".to_string());
    let rows = dedupe_rows(vec![a.clone(), b, c.clone()]);
    // gleicher Schlüssel (market, product, valid_from, region): erster gewinnt;
    // andere Region bleibt erhalten
    assert_eq!(rows, vec![a, c]);
}

#[test]
fn chain_detection_from_market() {
    let m = |id: &str, name: &str| Market::new(id, name);
    assert_eq!(chain_for(&m("LIDL_DE", "Lidl Deutschland")), Some("Lidl"));
    // EDEKA-Vertriebsmarken ohne "edeka" im Namen (ID ist nur numerisch)
    assert_eq!(chain_for(&m("022745", "E center Peltzer")), Some("EDEKA"));
    assert_eq!(chain_for(&m("4711", "Marktkauf Dresden")), Some("EDEKA"));
    assert_eq!(chain_for(&m("ALDI_NORD_DE", "ALDI Nord Deutschland")), Some("ALDI Nord"));
    assert_eq!(chain_for(&m("ALDI_SUED_DE", "ALDI Süd Deutschland")), Some("ALDI SÜD"));
    assert_eq!(chain_for(&m("831971", "REWE Christian Koehler oHG")), Some("REWE"));
    assert_eq!(chain_for(&m("1234", "Kaufland Dresden")), Some("Kaufland"));
    assert_eq!(chain_for(&m("42", "Feinkost Meier")), None);
}

#[test]
fn base_price_units_module_roundtrip() {
    // Absicherung, dass push dieselbe Ableitung nutzt wie compare/suggest
    let up = units::derive_unit_price(Some(0.99), &[Some("je 500-g-Packg.")]).unwrap();
    assert!((up.eur - 1.98).abs() < 1e-9);
}

// ---------------------------------------------------------- HTTP-Schicht

#[derive(Debug, Clone)]
struct Req {
    method: String,
    target: String, // Pfad + Query
    headers: Vec<(String, String)>,
    body: String,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Minimaler HTTP/1.1-Mock: nimmt Requests an, protokolliert sie und
/// antwortet immer mit 200 `[]`.
fn spawn_mock() -> (String, Arc<Mutex<Vec<Req>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let log: Arc<Mutex<Vec<Req>>> = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let mut parts = line.split_whitespace();
                let (Some(method), Some(target)) = (parts.next(), parts.next()) else { break };
                let (method, target) = (method.to_string(), target.to_string());
                let mut headers = Vec::new();
                let mut content_length = 0usize;
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) == 0 {
                        break;
                    }
                    let h = h.trim_end().to_string();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        let (k, v) = (k.trim().to_string(), v.trim().to_string());
                        if k.eq_ignore_ascii_case("content-length") {
                            content_length = v.parse().unwrap_or(0);
                        }
                        headers.push((k, v));
                    }
                }
                let mut body = vec![0u8; content_length];
                if content_length > 0 {
                    reader.read_exact(&mut body).unwrap();
                }
                log2.lock().unwrap().push(Req {
                    method,
                    target,
                    headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let resp = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n[]";
                if reader.get_mut().write_all(resp.as_bytes()).is_err() {
                    break;
                }
            }
        }
    });
    (format!("http://{addr}"), log)
}

/// Wie spawn_mock, antwortet aber immer mit dem angegebenen Fehlerstatus.
fn spawn_failing_mock(status: u16, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = [0u8; 65536];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 {status} ERR\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    format!("http://{addr}")
}

fn temp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("smartshop-push-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("test.db").to_string_lossy().into_owned()
}

/// DB mit einem REWE-Markt und `n` bepreisten Angeboten (valid_from 2026-07-13)
/// plus einem Altwochen-Angebot ohne Preis anlegen.
fn seed_db(path: &str, n: usize) {
    let _ = std::fs::remove_file(path);
    let conn = db::open(path).unwrap();
    db::upsert_market(&conn, &Market::new("m1", "REWE Christian Koehler oHG"))
        .unwrap();
    for i in 0..n {
        db::upsert_offer(&conn, &offer(&format!("Produkt {i:03}"), Some(1.0 + i as f64 / 100.0)))
            .unwrap();
    }
    db::upsert_offer(&conn, &offer("Ohne Preis", None)).unwrap();
}

fn run_push(db_path: &str, base_url: &str) -> anyhow::Result<()> {
    let opts = PushOptions {
        db_path: db_path.to_string(),
        chain: None,
        region: Some("01219".to_string()),
        dry_run: false,
        mirror_images: false,
    };
    let cfg = PushConfig { base_url: base_url.to_string(), api_key: "test-key".to_string() };
    push::run(&opts, Some(&cfg))
}

#[test]
fn push_batches_deletes_and_upserts() {
    let db_path = temp_db("batch");
    seed_db(&db_path, 150);
    let (base_url, log) = spawn_mock();

    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    // 1x DELETE (stale), 2x POST offers (100 + 50), 2x POST price_history,
    // 1x POST regions
    assert_eq!(reqs.len(), 6, "Requests: {reqs:#?}");

    let del = &reqs[0];
    assert_eq!(del.method, "DELETE");
    assert!(del.target.starts_with("/rest/v1/offers?"), "{}", del.target);
    assert!(del.target.contains("market=eq.REWE"), "{}", del.target);
    // Nur die gepushte Region aufräumen — andere Regionen bleiben unberührt.
    assert!(del.target.contains("region=eq.01219"), "{}", del.target);
    // Löscht alte Wochen UND Legacy-Zeilen ohne valid_from (URL-encodiert:
    // or=(valid_from.lt.2026-07-13,valid_from.is.null))
    let decoded = del.target.replace("%28", "(").replace("%29", ")").replace("%2C", ",");
    assert!(
        decoded.contains("or=(valid_from.lt.2026-07-13,valid_from.is.null)"),
        "{}",
        del.target
    );
    assert_eq!(del.header("apikey"), Some("test-key"));
    assert_eq!(del.header("authorization"), Some("Bearer test-key"));

    let (b1, b2) = (&reqs[1], &reqs[2]);
    for b in [b1, b2] {
        assert_eq!(b.method, "POST");
        assert!(b.target.starts_with("/rest/v1/offers?"), "{}", b.target);
        assert!(b.target.contains("on_conflict=market%2Cproduct%2Cvalid_from%2Cregion")
                || b.target.contains("on_conflict=market,product,valid_from,region"),
                "{}", b.target);
        assert_eq!(b.header("prefer"), Some("resolution=merge-duplicates"));
    }
    let rows1: Vec<SupabaseRow> = parse_rows(&b1.body);
    let rows2: Vec<SupabaseRow> = parse_rows(&b2.body);
    assert_eq!(rows1.len(), 100);
    assert_eq!(rows2.len(), 50);
    assert!(rows1.iter().all(|r| r.market == "REWE" && r.region.as_deref() == Some("01219")));

    let reg = &reqs[5];
    assert_eq!(reg.method, "POST");
    assert!(reg.target.starts_with("/rest/v1/regions?"), "{}", reg.target);
    assert!(reg.target.contains("on_conflict=plz"), "{}", reg.target);
    let v: serde_json::Value = serde_json::from_str(&reg.body).unwrap();
    assert_eq!(v[0]["plz"], "01219");
    assert!(v[0]["last_synced"].as_str().unwrap().starts_with("20"));
}

fn parse_rows(body: &str) -> Vec<SupabaseRow> {
    let v: serde_json::Value = serde_json::from_str(body).unwrap();
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| SupabaseRow {
            market: r["market"].as_str().unwrap().to_string(),
            product: r["product"].as_str().unwrap().to_string(),
            price: r["price"].as_f64().unwrap(),
            regular_price: r["regular_price"].as_f64(),
            unit: r["unit"].as_str().unwrap().to_string(),
            category: r["category"].as_str().map(String::from),
            emoji: None,
            image_url: r["image_url"].as_str().map(String::from),
            valid_from: r["valid_from"].as_str().map(String::from),
            valid_until: r["valid_until"].as_str().map(String::from),
            base_price: r["base_price"].as_f64(),
            base_unit: r["base_unit"].as_str().map(String::from),
            brand: None,
            ean: None,
            source: r["source"].as_str().unwrap().to_string(),
            region: r["region"].as_str().map(String::from),
        })
        .collect()
}

// Regression (Lidl, Juli 2026): Angebote ohne valid_from dürfen nie nach
// Supabase — die App filtert sie serverseitig weg und der Upsert-Schlüssel
// (market, product, valid_from, region) ist nicht NULL-sicher, jeder Lauf
// würde sie duplizieren.
#[test]
fn push_skips_offers_without_valid_from() {
    let db_path = temp_db("nodate");
    seed_db(&db_path, 3);
    {
        let conn = db::open(&db_path).unwrap();
        let mut dateless = offer("Ohne Datum", Some(2.49));
        dateless.valid_from = None;
        db::upsert_offer(&conn, &dateless).unwrap();
    }
    let (base_url, log) = spawn_mock();

    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    let upserted: Vec<SupabaseRow> = reqs
        .iter()
        .filter(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers?"))
        .flat_map(|r| parse_rows(&r.body))
        .collect();
    assert_eq!(upserted.len(), 3, "nur die 3 datierten Angebote");
    assert!(upserted.iter().all(|r| r.valid_from.is_some()));
    assert!(!upserted.iter().any(|r| r.product == "Ohne Datum"));
}

#[test]
fn push_fails_with_german_error_on_http_error() {
    let db_path = temp_db("err");
    seed_db(&db_path, 2);
    let base_url = spawn_failing_mock(401, "{\"message\":\"Invalid API key\"}");

    let err = run_push(&db_path, &base_url).unwrap_err().to_string();
    assert!(err.contains("fehlgeschlagen"), "{err}");
    assert!(err.contains("401"), "{err}");
    assert!(err.contains("Invalid API key"), "{err}");
}

#[test]
fn push_requires_region_unless_dry_run() {
    let db_path = temp_db("region");
    seed_db(&db_path, 1);
    let opts = PushOptions { db_path, chain: None, region: None, dry_run: false, mirror_images: false };
    let err = push::run(&opts, None).unwrap_err().to_string();
    assert!(err.contains("--region"), "{err}");
}

#[test]
fn dry_run_makes_no_requests() {
    let db_path = temp_db("dry");
    seed_db(&db_path, 3);
    let (_base_url, log) = spawn_mock();
    let opts = PushOptions { db_path, chain: None, region: None, dry_run: true, mirror_images: true };
    // cfg: None — Dry-Run braucht weder Env noch Netzwerk (auch nicht fürs Spiegeln)
    push::run(&opts, None).unwrap();
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn push_mirrors_images_and_caches_them() {
    let db_path = temp_db("mirror");
    let (base_url, log) = spawn_mock();

    // REWE-Markt + ein Angebot MIT Bild; die Bild-URL zeigt auf den Mock, damit
    // der Download klappt.
    let _ = std::fs::remove_file(&db_path);
    let conn = db::open(&db_path).unwrap();
    db::upsert_market(&conn, &Market::new("m1", "REWE Christian Koehler oHG"))
        .unwrap();
    let mut o = offer("Gouda", Some(1.99));
    o.images = vec![format!("{base_url}/img/gouda.jpg")];
    db::upsert_offer(&conn, &o).unwrap();
    drop(conn);

    let cfg = PushConfig { base_url: base_url.clone(), api_key: "k".to_string() };
    let opts = PushOptions {
        db_path: db_path.clone(),
        chain: None,
        region: Some("01219".to_string()),
        dry_run: false,
        mirror_images: true,
    };

    // Erster Lauf: Bild laden und in den Bucket hochladen.
    push::run(&opts, Some(&cfg)).unwrap();
    let reqs = log.lock().unwrap().clone();

    assert!(
        reqs.iter().any(|r| r.method == "GET" && r.target == "/img/gouda.jpg"),
        "Bild-Download fehlt: {reqs:#?}"
    );
    let upload = reqs
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/storage/v1/object/offer-images/"))
        .expect("Storage-Upload fehlt");
    assert_eq!(upload.header("x-upsert"), Some("true"));
    assert_eq!(upload.header("apikey"), Some("k"));

    let offers_post = reqs
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers"))
        .expect("Offers-Upsert fehlt");
    assert!(
        offers_post.body.contains("/storage/v1/object/public/offer-images/"),
        "image_url zeigt nicht auf den Bucket: {}",
        offers_post.body
    );

    // Zweiter Lauf: Cache-Treffer -> weder erneuter Download noch Upload.
    log.lock().unwrap().clear();
    push::run(&opts, Some(&cfg)).unwrap();
    let reqs2 = log.lock().unwrap().clone();
    assert!(
        !reqs2.iter().any(|r| r.target.starts_with("/storage/v1/object/offer-images/")),
        "Bild wurde erneut hochgeladen: {reqs2:#?}"
    );
    assert!(
        !reqs2.iter().any(|r| r.target == "/img/gouda.jpg"),
        "Bild wurde erneut geladen"
    );
    // Die Zeile trägt trotzdem die Bucket-URL.
    let offers_post2 = reqs2
        .iter()
        .find(|r| r.method == "POST" && r.target.starts_with("/rest/v1/offers"))
        .unwrap();
    assert!(offers_post2.body.contains("/storage/v1/object/public/offer-images/"));
}

#[test]
fn chain_filter_limits_push() {
    let db_path = temp_db("filter");
    seed_db(&db_path, 2);
    let (base_url, log) = spawn_mock();
    let opts = PushOptions {
        db_path,
        chain: Some("Lidl".to_string()), // DB enthält nur REWE
        region: Some("01219".to_string()),
        dry_run: false,
        mirror_images: false,
    };
    let cfg = PushConfig { base_url, api_key: "k".to_string() };
    push::run(&opts, Some(&cfg)).unwrap();
    assert!(log.lock().unwrap().is_empty());
}

// ------------------------------------------------------- Preis-Historie

/// Wie spawn_mock, antwortet aber auf Targets mit dem angegebenen Präfix mit
/// `fail_status` statt 200 — für Tests, die einen Teil-Ausfall simulieren.
fn spawn_selective_mock(
    fail_prefix: &'static str,
    fail_status: u16,
) -> (String, Arc<Mutex<Vec<Req>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let log: Arc<Mutex<Vec<Req>>> = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let mut parts = line.split_whitespace();
                let (Some(method), Some(target)) = (parts.next(), parts.next()) else { break };
                let (method, target) = (method.to_string(), target.to_string());
                let mut headers = Vec::new();
                let mut content_length = 0usize;
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) == 0 {
                        break;
                    }
                    let h = h.trim_end().to_string();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        let (k, v) = (k.trim().to_string(), v.trim().to_string());
                        if k.eq_ignore_ascii_case("content-length") {
                            content_length = v.parse().unwrap_or(0);
                        }
                        headers.push((k, v));
                    }
                }
                let mut body = vec![0u8; content_length];
                if content_length > 0 {
                    reader.read_exact(&mut body).unwrap();
                }
                let fail = target.starts_with(fail_prefix);
                log2.lock().unwrap().push(Req {
                    method,
                    target,
                    headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let resp = if fail {
                    format!("HTTP/1.1 {fail_status} ERR\r\ncontent-length: 2\r\n\r\n{{}}")
                } else {
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n[]"
                        .to_string()
                };
                if reader.get_mut().write_all(resp.as_bytes()).is_err() {
                    break;
                }
            }
        }
    });
    (format!("http://{addr}"), log)
}

fn history_posts(reqs: &[Req]) -> Vec<&Req> {
    reqs.iter()
        .filter(|r| r.method == "POST" && r.target.starts_with("/rest/v1/price_history"))
        .collect()
}

#[test]
fn push_sends_history_rows() {
    let db_path = temp_db("hist");
    seed_db(&db_path, 150);
    let (base_url, log) = spawn_mock();

    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    let hist = history_posts(&reqs);
    // 150 Zeilen in Batches à 100
    assert_eq!(hist.len(), 2, "History-Requests: {reqs:#?}");
    let rows: Vec<serde_json::Value> = hist
        .iter()
        .flat_map(|r| {
            serde_json::from_str::<serde_json::Value>(&r.body)
                .unwrap()
                .as_array()
                .unwrap()
                .clone()
        })
        .collect();
    assert_eq!(rows.len(), 150);
    let first = &rows[0];
    assert_eq!(first["market"], "REWE");
    assert_eq!(first["region"], "01219");
    assert_eq!(first["valid_from"], "2026-07-13");
    assert!(first["price"].is_number());
    // Nur die Historien-Spalten — keine Anzeige-Felder wie image_url/emoji
    assert!(first.get("image_url").is_none(), "{first}");
    assert!(first.get("emoji").is_none(), "{first}");
    assert!(first.get("source").is_none(), "{first}");
}

#[test]
fn history_upsert_headers() {
    let db_path = temp_db("hist-headers");
    seed_db(&db_path, 2);
    let (base_url, log) = spawn_mock();

    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    let hist = history_posts(&reqs);
    assert_eq!(hist.len(), 1);
    let h = hist[0];
    assert!(
        h.target.contains("on_conflict=market%2Cproduct%2Cregion%2Cvalid_from")
            || h.target.contains("on_conflict=market,product,region,valid_from"),
        "{}",
        h.target
    );
    assert_eq!(h.header("prefer"), Some("resolution=merge-duplicates"));
    assert_eq!(h.header("apikey"), Some("test-key"));
    assert_eq!(h.header("authorization"), Some("Bearer test-key"));
}

#[test]
fn history_skips_null_price() {
    let db_path = temp_db("hist-null");
    // seed_db legt zusätzlich ein Angebot "Ohne Preis" (price: None) an
    seed_db(&db_path, 3);
    let (base_url, log) = spawn_mock();

    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    let hist = history_posts(&reqs);
    assert_eq!(hist.len(), 1);
    let rows: serde_json::Value = serde_json::from_str(&hist[0].body).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 3);
    assert!(!hist[0].body.contains("Ohne Preis"), "{}", hist[0].body);
}

#[test]
fn history_failure_does_not_fail_push() {
    let db_path = temp_db("hist-fail");
    seed_db(&db_path, 2);
    let (base_url, log) = spawn_selective_mock("/rest/v1/price_history", 500);

    // Offers-Push muss trotz kaputter Historie erfolgreich durchlaufen …
    run_push(&db_path, &base_url).unwrap();

    let reqs = log.lock().unwrap().clone();
    // … der Historie-Request wurde versucht …
    assert_eq!(history_posts(&reqs).len(), 1, "{reqs:#?}");
    // … und der Regions-Upsert danach fand trotzdem statt.
    assert!(
        reqs.iter().any(|r| r.method == "POST" && r.target.starts_with("/rest/v1/regions")),
        "{reqs:#?}"
    );
}
