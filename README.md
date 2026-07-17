# smartshop

Ein CLI-Werkzeug, das Angebote deutscher Supermärkte scrapt, in einer
SQLite-Datenbank speichert und durchsuch-, vergleich- und beobachtbar macht.
Acht Ketten werden unterstützt (Rewe, Penny, Kaufland, Lidl, Netto, ALDI Nord,
ALDI Süd, EDEKA). Die CLI-Ausgabe ist deutsch.

## Bauen & Installieren

Rust (Edition 2024) wird benötigt:

```sh
cargo build --release
# Binary: target/release/smartshop
```

Für die Rewe-Scraper zusätzlich das `rewerse`-CLI und ein Client-Zertifikat —
siehe [docs/rewe-cert.md](docs/rewe-cert.md). Netto, ALDI Süd und EDEKA rufen
das System-`curl` auf (Akamai blockt reqwest), `curl` muss also im `PATH` sein.

Jeder Befehl nimmt `--db <pfad>` (Standard `smartshop.db` im Arbeitsverzeichnis).
Die Datenbank wird bei Bedarf angelegt und migriert.

## Schnellstart

Angebote abrufen und speichern:

```sh
smartshop fetch --store lidl --zip 50667
# Suche Lidl-Markt für PLZ 50667...
# Markt gefunden: Lidl Deutschland (ID: LIDL_DE)
# Lade Angebote...
# 470 Angebote gefunden.
# 470 Angebote in 'smartshop.db' gespeichert.
```

Alle Ketten auf einmal (`--all-stores`, mit Zusammenfassung pro Kette; einzelne
Fehler brechen den Lauf nicht ab):

```sh
smartshop fetch --all-stores --zip 50667
```

`--dry-run` gibt nur aus, ohne zu speichern; `--notify` prüft nach dem Abruf die
Watchlist.

## Befehle

### search — Angebote nach Titel durchsuchen

```sh
smartshop search Butter
#   [Wochen-Angebote] Butter (250-g-Packung) — 1.29 €  [2026-07-13 – 2026-07-18]
#   ...
```

Optional `--max-price <euro>`.

### compare — Preis eines Produkts über alle Märkte

```sh
smartshop compare Milch
# MÜLLER Reine Buttermilch*
#   Penny Am Eigelstein    0.66 €  (1.32 €/kg) (je 500 g)
#   ...
```

Gruppiert nach normalisiertem Produktnamen, günstigster Markt zuerst, mit
Grundpreis wo ableitbar.

### stats — Statistik pro Markt + Top-Rabatte

```sh
smartshop stats
# Angebote pro Markt:
#   Filiale                      Angebote  Gültigkeit              Ø Rabatt
#   Penny Am Eigelstein               542  2026-07-13 – 2026-07-26 33 %
#   ...
# Top 10 Rabatte:
#    1. -80 %  UNCLE SAM Herren-T-Shirt* je Stück — 3.99 € statt 19.99 € (Penny Am Eigelstein)
```

### history — Preisverlauf eines Produkts

```sh
smartshop history Butter
```

Zeigt je Titel/Markt die über die Zeit gesehenen Preise (aus `price_history`).

### deals — Preissenkungen

```sh
smartshop deals            # alle erfassten Senkungen
smartshop deals --since 7  # nur der letzten 7 Tage
```

### watch — Watchlist (cron-tauglich)

```sh
smartshop watch add Kaffee --max-price 5
# Watch #1 angelegt: 'Kaffee' (bis 5.00 €)
smartshop watch list
smartshop watch check   # druckt Treffer, Exit-Code 1 wenn es welche gibt
smartshop watch remove 1
```

`watch check` endet mit **Exit-Code 1**, sobald mindestens ein Watch anschlägt
(sonst 0) — siehe [docs/cron.md](docs/cron.md) für Benachrichtigungen.

### list — Einkaufsliste

```sh
smartshop list add Butter
smartshop list show
smartshop list suggest
# Butter                   0.66 € bei Penny Am Eigelstein — MÜLLER Reine Buttermilch*  (1.32 €/kg)
smartshop list remove Butter
smartshop list clear
```

`suggest` findet je Artikel das günstigste passende Angebot über alle Märkte.

### export — JSON oder CSV

```sh
smartshop export --format csv --query Butter > butter.csv
smartshop export --format json --out angebote.json
```

Ohne `--out` geht die Ausgabe auf stdout; `--query` filtert nach Titel/Untertitel.

### serve — Web-Dashboard + lesende JSON-API

```sh
smartshop serve --port 8080
# Web-UI läuft auf http://0.0.0.0:8080, JSON-API auf http://0.0.0.0:8080/api (DB: smartshop.db)
```

Das Web-Dashboard liegt auf `/`, die JSON-Endpoints unter `/api/*`
(siehe unten und [docs/cron.md](docs/cron.md)).

### push — Supabase-Push

Lädt die gespeicherten Angebote in die Supabase-Tabelle `public.offers`
(PostgREST-Upsert auf `market,product,valid_from,region`, Batches à 100) und
trägt die PLZ in `public.regions` ein. Veraltete Wochen des jeweiligen Markts
werden vorher gelöscht. Angebote ohne Preis werden übersprungen.

```sh
export SUPABASE_URL="https://xyz.supabase.co"
export SUPABASE_SERVICE_KEY="…"   # Service-Role-Key, nicht der anon key

smartshop push --region 01219                  # alle Märkte
smartshop push --store lidl --region 01219     # nur eine Kette
smartshop push --dry-run                       # nur zeigen, kein Netzwerk
```

`--region <PLZ>` ist Pflicht (außer bei `--dry-run`). Für den wöchentlichen
Sync per Cron einfach nach dem Abruf pushen:

```cron
0 6 * * 1 SMARTSHOP_ZIP=01219 /pfad/zu/scripts/fetch-all.sh && \
  SUPABASE_URL=… SUPABASE_SERVICE_KEY=… smartshop push --region 01219 --db ~/.local/share/smartshop/smartshop.db
```

## Scraper-Unterstützung

| Kette | Auth nötig | Markt | Angebote (ballpark) |
|---|---|---|---|
| Rewe | ja — TLS-Cert + `rewerse` | filialspezifisch (PLZ) | variiert je Filiale |
| Penny | nein | filialspezifisch (PLZ) | ~540 |
| Kaufland | nein | filialspezifisch (PLZ) | variiert (nicht jede PLZ) |
| Lidl | nein | national | ~470 |
| Netto | nein (curl) | filialspezifisch (PLZ) | ~190 |
| ALDI Nord | nein | national | ~240 |
| ALDI Süd | nein (curl) | national | ~75 |
| EDEKA | nein (curl) | filialspezifisch (PLZ) | variiert (nicht jede Region) |

Ballpark-Zahlen aus einem Live-Abruf für PLZ 50667 (Köln) am 2026-07-17;
tatsächliche Zahlen schwanken pro Woche und Filiale. Kaufland und EDEKA lieferten
für diese PLZ keinen Treffer — beide sind regionsabhängig. Rewe wurde mangels
Zertifikat nicht live gemessen.

## HTTP-API-Endpoints

Alle Endpoints sind `GET` und liefern JSON; Fehler als `{"error": "..."}` mit
Status 400 (fehlender/ungültiger Parameter) oder 500. Über `smartshop serve`
sind sie unter dem Präfix **`/api`** erreichbar (z. B. `/api/offers?q=Butter`);
die Wurzelpfade gehören dem Web-Dashboard.

| Endpoint | Parameter | Liefert |
|---|---|---|
| `/markets` | – | Gespeicherte Filialen |
| `/offers` | `q` (Pflicht), `max_price`, `market` | Angebote zur Suche |
| `/compare` | `q` (Pflicht) | Preisvergleich, gruppiert pro Produkt |
| `/stats` | – | Angebote/Filiale + Top-10-Rabatte |
| `/history` | `q` (Pflicht) | Preisverlauf |
| `/deals` | `since` (Tage, optional) | Preissenkungen |
| `/watches` | – | Watchlist |
| `/watches/check` | – | `{"hits": bool, "watches": [...]}` |
| `/list` | – | Einkaufsliste |
| `/list/suggest` | – | Günstigster Markt je Listen-Artikel |

Die API ist rein lesend und **ohne Authentifizierung** — nur im vertrauten Netz
betreiben.

## Web-Dashboard

`smartshop serve` liefert zusätzlich ein server-gerendertes HTML-Dashboard —
komplett ohne JavaScript, CSS eingebettet, keine externen Assets.

| Seite | Inhalt |
|---|---|
| `/` | Übersicht: Angebote pro Markt, Gültigkeitszeiträume, Top-Rabatte |
| `/search?q=…` | Angebotssuche mit Markt, Preis und Grundpreis |
| `/compare?q=…` | Preisvergleich pro Produkt über alle Märkte, günstigster zuerst |
| `/watchlist` | Beobachtungen anzeigen, anlegen und entfernen (POST-Formulare) |
| `/history?offer=…` | Preisverlauf als Inline-SVG-Sparkline plus Tabelle |

Wie die JSON-API ist das Dashboard ohne Authentifizierung — nur im vertrauten
Netz betreiben. Schreibend ist einzig die Watchlist (anlegen/entfernen).

## Datenbank & Schema

SQLite im WAL-Modus. Tabellen: `markets`, `offers`, `price_history`, `watches`,
`shopping_list`. Die Schema-Version steht in `PRAGMA user_version`; `db::open()`
migriert beim Öffnen automatisch auf die aktuelle Version. Schema-Änderungen
erhöhen `SCHEMA_VERSION` und ergänzen einen Migrationsschritt in `migrate()`
(`src/db.rs`) — bestehende Datenbanken werden dabei in-place aktualisiert.

## Dokumentation

- [docs/rewe-cert.md](docs/rewe-cert.md) — Rewe-TLS-Zertifikat einrichten
- [docs/cron.md](docs/cron.md) — Cron/launchd-Automatisierung und JSON-API
- [scripts/fetch-all.sh](scripts/fetch-all.sh) — cron-fertiger Abruf-Wrapper
