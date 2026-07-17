# Scraper-Referenz

Stand: 2026-07 (KW 29), verifiziert mit PLZ 01219 (Dresden). Typische
Angebotszahlen schwanken je Woche und Region.

## Endpoints pro Kette

| Kette | Quelle / URL-Muster | Auth / Eigenheiten | Markt-Bezug | Typisch/Woche | Fixture |
|---|---|---|---|---|---|
| REWE | `rewerse`-CLI (mobile API) | **mTLS**: Client-Zertifikat aus der REWE-App nötig (`cert.pem` + `private.key`, siehe [docs/rewe-cert.md](../rewe-cert.md)) | filialspezifisch (PLZ → marketId) | variiert je Filiale | `tests/fixtures/rewe/discounts.json` (handgebaut im rewerse-Format) |
| Penny | `penny.de/.rest/market`, Kategorien aus `/angebote`-HTML, dann `/.rest/offers/by-category/<JAHR-WOCHE>/<kategorie>?region=<sellingRegion>` | nur Browser-User-Agent; Aktionspreise als String mit Fußnoten-Sternchen (`"0.49*"`) | regional (`sellingRegion` des Markts) | ~550-600 (2 Wochen) | `tests/fixtures/penny/offers_kuehlregal.json` |
| Kaufland | `filiale.kaufland.de/.klstorefinder.json` + server-seitig gerendertes `/angebote/uebersicht.html` | Filiale über Cookie `x-aem-variant=<id>`; **Titel = Marke, Produkt im Untertitel** (Offer-ID enthält deshalb den Untertitel); **dasselbe Angebot erscheint in mehreren Kategorien** (Warengruppe + „Unsere Knüller" etc.) — Dedup erst beim DB-Upsert über die ID | filialspezifisch | ~650 (inkl. Kategorie-Duplikate) | `tests/fixtures/kaufland/uebersicht.html` |
| Lidl | `lidl.de/q/api/search?...&store=1` (paginiert) | `Accept: application/json` Pflicht (sonst 406); **Lidl-Plus-Angebote** tragen den Preis in `lidlPlus[0].price` statt `price` | bundesweit (synthetischer Markt `LIDL_DE`) | ~450-480 | `tests/fixtures/lidl/search_store1.json` |
| EDEKA | `edeka.de/api/marketsearch/markets?searchstring=<PLZ>`, Markt-ID via 308-Redirect der Legacy-URL, Angebote aus `/maerkte/<id>/angebote/`-HTML | Akamai-Bot-Schutz → System-`curl` (util.rs); Preis maschinenlesbar im `sr-only`-Div („Festpreis von 3.99 €" / „App-Preis von …") | filialspezifisch | ~200 | `tests/fixtures/edeka/angebote.html` |
| Netto | Intershop-Filialsuche (JSON) + `/filialangebote/{1,2,4,5}`-HTML | Akamai → System-`curl`; Filiale über Cookie `netto_user_stores_id` | filialspezifisch | ~300 | `tests/fixtures/netto/filialangebote_1.html` |
| ALDI Nord | `aldi-nord.de/angebote.html`, Daten im `__NEXT_DATA__`-JSON (`OFFER_GET.res.algoliaDataMap`) | plain reqwest | bundesweit (`ALDI_NORD_DE`) | ~230 | `tests/fixtures/aldi_nord/angebote.html` |
| ALDI Süd | `api.aldi-sued.de/v3/product-search?categoryKey=1588161426582123` (paginiert) | Akamai → System-`curl`; Preise in **Cent**; keine Gültigkeitsdaten | Süd-Gebiet einheitlich (`ALDI_SUED_DE`) | ~75 | `tests/fixtures/aldi_sued/product_search.json` |

## Bekannte NULL-Preise (diagnostiziert 2026-07)

- **EDEKA (~20-25/Woche): echt.** „Tagespreis"-Kacheln und reine
  PAYBACK-Extra-Punkte-Kacheln tragen weder in der Kachel noch im
  zugehörigen Dialog einen Preis. Sie kommen bewusst mit `price = NULL` an.
- **Lidl (~7/Woche): war ein Parser-Bug, gefixt.** Lidl-Plus-exklusive
  Angebote haben in `gridbox.data.price` nur eine leere Hülle; der Preis
  steht in `gridbox.data.lidlPlus[0].price`. Seit dem Fix wird er von dort
  gelesen (Regressionstest in `tests/scrapers.rs`).

## Gemeinsame Infrastruktur (`src/scrapers/util.rs`)

- `curl_get` / `curl_redirect_url`: System-`curl` mit vollem
  Browser-Header-Satz für Akamai-geschützte Hosts (Netto, ALDI Süd, EDEKA) —
  reqwest/rustls wird dort per TLS-Fingerprint mit 403 geblockt. 3 Versuche
  mit 3 s Abstand.
- `async_client` / `blocking_client`: reqwest-Clients mit gemeinsamem
  Browser-User-Agent (Penny, Lidl, Kaufland, ALDI Nord).
- `polite_pause(url)`: höfliches Rate-Limiting — vor aufeinanderfolgenden
  Requests an denselben Host eine zufällig gestreute Pause (300-800 ms).
- `ctx(kette, schritt, url)`: einheitlicher Fehlerkontext
  (`[Kette] Schritt fehlgeschlagen: URL`).

## Tests

- Offline: `cargo test` — Parser-Tests gegen die Fixtures in
  `tests/fixtures/<kette>/` (`tests/scrapers.rs` + Modul-Unit-Tests).
- Live: `cargo test --lib -- --ignored --nocapture --test-threads=1` —
  ein Live-Test pro Kette (außer REWE, braucht das Zertifikat), PLZ 01219.

Fixtures sind auf wenige repräsentative Angebote gekürzte Live-Antworten
vom 2026-07-17; das REWE-Fixture ist mangels Zertifikat handgebaut.
