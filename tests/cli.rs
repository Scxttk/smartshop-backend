use std::process::Command;

use smartshop::db;
use smartshop::models::{Market, Offer};

fn offer(id: &str, market_id: &str, title: &str, subtitle: Option<&str>, overline: Option<&str>, price: f64, regular: Option<f64>) -> Offer {
    Offer {
        id: id.to_string(),
        market_id: market_id.to_string(),
        title: title.to_string(),
        subtitle: subtitle.map(String::from),
        overline: overline.map(String::from),
        price: Some(price),
        regular_price: regular,
        category: Some("Test".to_string()),
        nutri_score: None,
        valid_from: Some("2026-07-13".to_string()),
        valid_until: Some("2026-07-19".to_string()),
        images: vec![],
        biozid: false,
        flyer_page: None,
    }
}

// Fixture-DB mit zwei Märkten und vergleichbaren Angeboten anlegen
fn build_fixture(path: &std::path::Path) {
    let conn = db::open(path.to_str().unwrap()).expect("DB öffnen");
    db::upsert_market(&conn, &Market { id: "M1".into(), name: "Testmarkt Eins".into() }).unwrap();
    db::upsert_market(&conn, &Market { id: "M2".into(), name: "Testmarkt Zwei".into() }).unwrap();

    // Gleiches Produkt in beiden Märkten, M2 billiger
    db::upsert_offer(&conn, &offer("o1", "M1", "MEGGLE Feine Butter", Some("je 250-g-Packg. (1 kg = 6.36)"), None, 1.59, Some(2.59))).unwrap();
    db::upsert_offer(&conn, &offer("o2", "M2", "MEGGLE Feine Butter", Some("je 250-g-Packg. (1 kg = 5.56)"), None, 1.39, Some(2.59))).unwrap();
    // Kaufland-Stil: Marke im Titel, Produkt im Untertitel, Menge im Overline
    db::upsert_offer(&conn, &offer("o3", "M1", "PFANNER", Some("Eistee"), Some("je 2-l-Packg. (1 l = 0.56)"), 1.11, Some(1.99))).unwrap();
    // Titel mit CSV-Sonderzeichen
    db::upsert_offer(&conn, &offer("o4", "M2", "Käse \"Extra\", gereift", Some("0.2 kg"), None, 2.99, None)).unwrap();
}

fn run(args: &[&str]) -> (String, String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_smartshop"))
        .args(args)
        .output()
        .expect("Binary ausführen");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

fn fixture_db(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("smartshop_cli_test_{name}_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    build_fixture(&path);
    path
}

#[test]
fn search_finds_offers_by_title() {
    let dbf = fixture_db("search");
    let (stdout, _, ok) = run(&["search", "Butter", "--db", dbf.to_str().unwrap()]);
    assert!(ok);
    assert!(stdout.contains("MEGGLE Feine Butter"), "stdout: {stdout}");
    assert!(stdout.contains("2 Treffer"), "stdout: {stdout}");
    // Billigstes zuerst (ORDER BY price)
    let pos139 = stdout.find("1.39 €").unwrap();
    let pos159 = stdout.find("1.59 €").unwrap();
    assert!(pos139 < pos159);
}

#[test]
fn compare_groups_and_sorts_cheapest_first() {
    let dbf = fixture_db("compare");
    let (stdout, _, ok) = run(&["compare", "Butter", "--db", dbf.to_str().unwrap()]);
    assert!(ok);
    // Eine Gruppe, günstigster Markt (Zwei) vor Markt Eins
    let pos2 = stdout.find("Testmarkt Zwei").expect("Markt Zwei fehlt");
    let pos1 = stdout.find("Testmarkt Eins").expect("Markt Eins fehlt");
    assert!(pos2 < pos1, "stdout: {stdout}");
    // Grundpreis aus expliziter Angabe
    assert!(stdout.contains("5.56 €/kg"), "stdout: {stdout}");
}

#[test]
fn compare_matches_subtitle_for_kaufland_style_offers() {
    let dbf = fixture_db("compare_sub");
    let (stdout, _, ok) = run(&["compare", "Eistee", "--db", dbf.to_str().unwrap()]);
    assert!(ok);
    assert!(stdout.contains("PFANNER Eistee"), "stdout: {stdout}");
    assert!(stdout.contains("0.56 €/l"), "stdout: {stdout}");
}

#[test]
fn export_json_roundtrips() {
    let dbf = fixture_db("json");
    let (stdout, _, ok) = run(&["export", "--format", "json", "--db", dbf.to_str().unwrap()]);
    assert!(ok);
    let offers: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("gültiges JSON");
    assert_eq!(offers.len(), 4);
    assert!(offers.iter().any(|o| o["title"] == "PFANNER"));
}

#[test]
fn export_csv_escapes_special_characters() {
    let dbf = fixture_db("csv");
    let (stdout, _, ok) = run(&["export", "--format", "csv", "--query", "Käse", "--db", dbf.to_str().unwrap()]);
    assert!(ok);
    let mut lines = stdout.lines();
    assert!(lines.next().unwrap().starts_with("id,market_id,title"));
    let row = lines.next().expect("Datenzeile");
    // Anführungszeichen verdoppelt, Feld mit Komma in Quotes
    assert!(row.contains("\"Käse \"\"Extra\"\", gereift\""), "row: {row}");
    assert_eq!(stdout.lines().filter(|l| !l.is_empty()).count(), 2);
}

#[test]
fn export_query_filters() {
    let dbf = fixture_db("filter");
    let (stdout, _, ok) = run(&["export", "--format", "json", "--query", "Butter", "--db", dbf.to_str().unwrap()]);
    assert!(ok);
    let offers: Vec<serde_json::Value> = serde_json::from_str(&stdout).unwrap();
    assert_eq!(offers.len(), 2);
}
