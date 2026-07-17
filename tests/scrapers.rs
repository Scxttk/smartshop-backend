// Offline-Parser-Tests gegen eingefrorene Fixtures (tests/fixtures/<chain>/).
// Die Fixtures sind auf wenige repräsentative Angebote gekürzte Live-Antworten
// vom 2026-07-17 (KW 29, PLZ 01219 Dresden). Laufen ohne Netz bei jedem
// `cargo test`.

use smartshop::scrapers;

// ---------------------------------------------------------------- Kaufland

#[test]
fn kaufland_fixture_parses_sections_and_tiles() {
    let offers =
        scrapers::kaufland::parse_offers(include_str!("fixtures/kaufland/uebersicht.html"), "DE7380")
            .unwrap();
    assert_eq!(offers.len(), 9);

    let lachs = &offers[0];
    assert_eq!(lachs.title, "K-BLUE BAY");
    assert_eq!(lachs.subtitle.as_deref(), Some("Lachsforellenfilet"));
    assert_eq!(lachs.price, Some(3.19));
    assert_eq!(lachs.regular_price, Some(3.79));
    assert_eq!(lachs.category.as_deref(), Some("Fisch"));
    assert_eq!(lachs.valid_from.as_deref(), Some("2026-07-16"));
    assert_eq!(lachs.valid_until.as_deref(), Some("2026-07-22"));
}

// Regression: Kaufland-Titel sind Marken, das Produkt steht im Untertitel —
// die Offer-ID muss den Untertitel enthalten, sonst kollidieren alle
// Angebote einer Marke (Fix vom 2026-07, Commit 7b695e2).
#[test]
fn kaufland_offer_ids_include_subtitle() {
    let offers =
        scrapers::kaufland::parse_offers(include_str!("fixtures/kaufland/uebersicht.html"), "DE7380")
            .unwrap();

    let bay: Vec<_> = offers.iter().filter(|o| o.title == "K-BLUE BAY").collect();
    assert_eq!(bay.len(), 2);
    assert_ne!(bay[0].id, bay[1].id, "gleiche Marke, anderes Produkt -> andere ID");
}

// Quirk: dasselbe Angebot erscheint in der Warengruppe UND in "Unsere
// Knüller". Beide Vorkommen teilen dieselbe ID (Dedup passiert beim
// DB-Upsert, nicht im Parser).
#[test]
fn kaufland_duplicate_listing_across_categories_shares_id() {
    let offers =
        scrapers::kaufland::parse_offers(include_str!("fixtures/kaufland/uebersicht.html"), "DE7380")
            .unwrap();

    let bauer: Vec<_> = offers.iter().filter(|o| o.title == "BAUER").collect();
    assert_eq!(bauer.len(), 2, "Duplikat aus zwei Kategorien erwartet");
    assert_eq!(bauer[0].id, bauer[1].id);
    assert_ne!(bauer[0].category, bauer[1].category);
}

// ---------------------------------------------------------------- EDEKA

#[test]
fn edeka_fixture_parses_offers_and_dates() {
    let offers =
        scrapers::edeka::parse_offers(include_str!("fixtures/edeka/angebote.html"), "022745")
            .unwrap();
    assert_eq!(offers.len(), 6);

    let blaubeeren = &offers[0];
    assert_eq!(blaubeeren.title, "Kulturheidelbeeren");
    assert_eq!(blaubeeren.price, Some(3.99));
    assert_eq!(blaubeeren.valid_from.as_deref(), Some("2026-07-13"));
    assert_eq!(blaubeeren.valid_until.as_deref(), Some("2026-07-18"));

    // App-Preis wird wie ein Festpreis geparst
    let butter = offers.iter().find(|o| o.title.starts_with("Meggle")).unwrap();
    assert_eq!(butter.price, Some(0.99));
}

// EDEKA-NULL-Preise sind echt: "Tagespreis"-Kacheln und reine
// PAYBACK-Punkte-Kacheln haben im HTML (Kachel + Dialog) keinen Preis.
// Sie kommen bewusst mit price = None an — kein Parser-Bug.
#[test]
fn edeka_priceless_promo_tiles_stay_price_none() {
    let offers =
        scrapers::edeka::parse_offers(include_str!("fixtures/edeka/angebote.html"), "022745")
            .unwrap();

    let tagespreis = offers.iter().find(|o| o.title.contains("Grillkäse")).unwrap();
    assert_eq!(tagespreis.price, None);

    let payback = offers.iter().find(|o| o.title == "Arla Kærgården").unwrap();
    assert_eq!(payback.price, None);

    assert_eq!(offers.iter().filter(|o| o.price.is_none()).count(), 2);
}

// ---------------------------------------------------------------- Penny

#[test]
fn penny_fixture_parses_all_tiles() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/penny/offers_kuehlregal.json")).unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut offers = Vec::new();
    scrapers::penny::parse_offer_tiles(
        &raw, "4030829", "kuehlregal", "2026-07-13", "2026-07-19", &mut seen, &mut offers,
    );
    assert_eq!(offers.len(), 6);

    let joghurt = &offers[0];
    assert_eq!(joghurt.title, "MÖVENPICK Feinjoghurt");
    assert_eq!(joghurt.price, Some(0.77));
    assert_eq!(joghurt.regular_price, Some(0.99));
    assert_eq!(joghurt.valid_from.as_deref(), Some("2026-07-13"));
    assert_eq!(joghurt.category.as_deref(), Some("kuehlregal"));
}

// Regression: Penny-Aktionspreise kommen als String mit Fußnoten-Sternchen
// ("0.49*") — json_price muss */€ abstreifen (Fix vom 2026-07, Commit 52d87ca).
#[test]
fn penny_star_prices_are_parsed() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/penny/offers_kuehlregal.json")).unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut offers = Vec::new();
    scrapers::penny::parse_offer_tiles(
        &raw, "4030829", "kuehlregal", "2026-07-13", "2026-07-19", &mut seen, &mut offers,
    );

    let buttermilch = offers.iter().find(|o| o.title == "MILPRIMA Reine Buttermilch").unwrap();
    assert_eq!(buttermilch.price, Some(0.49), "String-Preis \"0.49*\" muss geparst werden");
    assert_eq!(buttermilch.regular_price, Some(0.59));
    for o in &offers {
        assert!(o.price.is_some(), "Preis fehlt bei {}", o.title);
    }
}

// Kategorien-übergreifende Dedup: derselbe Titel darf nur einmal ankommen.
#[test]
fn penny_duplicate_titles_across_categories_are_deduped() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/penny/offers_kuehlregal.json")).unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut offers = Vec::new();
    scrapers::penny::parse_offer_tiles(
        &raw, "4030829", "top-angebote", "2026-07-13", "2026-07-19", &mut seen, &mut offers,
    );
    scrapers::penny::parse_offer_tiles(
        &raw, "4030829", "kuehlregal", "2026-07-13", "2026-07-19", &mut seen, &mut offers,
    );
    assert_eq!(offers.len(), 6, "zweite Kategorie mit denselben Titeln darf nichts hinzufügen");
}

// ---------------------------------------------------------------- Lidl

#[test]
fn lidl_fixture_parses_all_tiles() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/lidl/search_store1.json")).unwrap();
    let items = raw["items"].as_array().unwrap();
    assert_eq!(items.len(), 6);

    let offers: Vec<_> = items
        .iter()
        .filter_map(|it| scrapers::lidl::parse_tile(it, "LIDL_DE"))
        .collect();
    assert_eq!(offers.len(), 6, "jedes Tile muss ein Offer ergeben");

    let prosecco = &offers[0];
    assert_eq!(prosecco.title, "ALLINI Prosecco Treviso DOC Vino Frizzante trocken, Perlwein");
    assert_eq!(prosecco.price, Some(3.49));
}

// Regression: Lidl-Plus-exklusive Angebote haben in data.price nur eine leere
// Hülle — der Preis steht in data.lidlPlus[0].price. Vor dem Fix kamen diese
// ~7 Angebote pro Woche mit price = NULL an.
#[test]
fn lidl_plus_only_offers_get_price_from_lidl_plus_block() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/lidl/search_store1.json")).unwrap();
    let items = raw["items"].as_array().unwrap();

    let schrank = scrapers::lidl::parse_tile(&items[4], "LIDL_DE").unwrap();
    assert_eq!(schrank.title, "LIVARNO Waschbeckenunterschrank");
    assert_eq!(schrank.price, Some(19.99));
    assert_eq!(schrank.regular_price, Some(39.99));
    // "ü" kommt in der API NFD-zerlegt (u + kombinierender Umlaut)
    assert_eq!(schrank.subtitle.as_deref(), Some("Je Stu\u{308}ck"));

    for it in items {
        let o = scrapers::lidl::parse_tile(it, "LIDL_DE").unwrap();
        assert!(o.price.is_some(), "Preis fehlt bei {}", o.title);
    }
}
