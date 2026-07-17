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

Für den regelmäßigen Sync übernimmt `scripts/nightly.sh` sync-regions +
Watchlist-Check in einem Rutsch; auf macOS installiert
`scripts/install-launchd.sh` den fertigen Nacht-Agent — siehe
[docs/automation.md](docs/automation.md).

Beim Push wird jedes Angebot deterministisch angereichert (`src/enrich.rs`):
`category` enthält eine von 15 festen Kategorien (statt der rohen
Scraper-Kategorie), `emoji` ein passendes Emoji aus einer kuratierten
Keyword-Tabelle — ohne Treffer das Standard-Emoji der Kategorie, nie null.
Die App hardcodet diese Liste; Änderungen nur zusammen mit einem App-Update
(Regressionstest in `tests/enrich.rs`).

Zusätzlich trägt jede Zeile in `image_url` ein Produktbild: Die Scraper liefern
die Händler-Bild-URLs mit, der Push spiegelt sie in den öffentlichen
Supabase-Storage-Bucket `offer-images` (`src/storage.rs`) und schreibt die
stabile Bucket-URL in die Zeile — Händler-CDNs rotieren ihre Pfade wöchentlich
und blocken teils Hotlinks. Die Spiegelung ist idempotent (Objektpfad =
sha256 der Quell-URL, Upload mit `x-upsert`); bereits hochgeladene Bilder merkt
sich die lokale Tabelle `uploaded_images`, sodass Nachtläufe nur neue Bilder
anfassen. Fehler einzelner Bilder brechen den Push nicht ab — dann bleibt die
Händler-URL stehen, das Emoji ist der letzte Fallback in der App.
`--no-mirror-images` schaltet die Spiegelung ab.

| Kategorie | Default-Emoji | | Kategorie | Default-Emoji |
|---|---|---|---|---|
| Obst & Gemüse | 🥬 | | Alkohol | 🍺 |
| Molkerei & Eier | 🥛 | | Vorräte & Kochen | 🥫 |
| Fleisch & Wurst | 🥩 | | Drogerie | 🧴 |
| Fisch | 🐟 | | Haushalt | 🧽 |
| Backwaren | 🥖 | | Tierbedarf | 🐾 |
| Tiefkühl | ❄️ | | Kinder | 🧸 |
| Süßes & Snacks | 🍬 | | Sonstiges | 🛒 |
| Getränke | 🥤 | | | |

### sync-regions — Multi-Region-Sync

Liest alle aktiven Regionen aus der Supabase-Tabelle `public.regions`
(sortiert nach `requested_at`, älteste Anfrage zuerst) und macht pro PLZ den
kompletten Durchlauf: alle Ketten fetchen, gefundene Filialen nach
`public.markets` melden, Angebote mit `--region <PLZ>` pushen. Die lokale
`offers`-Tabelle wird pro Region geleert, damit keine Angebote fremder
Regionen mitgepusht werden.

```sh
export SUPABASE_URL="https://xyz.supabase.co"
export SUPABASE_SERVICE_KEY="…"

smartshop sync-regions                          # bis zu 10 Regionen
smartshop sync-regions --max-regions 3          # Limit ändern
smartshop sync-regions --dry-run                # keine Supabase-Writes
```

Fehler einzelner Regionen brechen den Lauf nicht ab; Exit-Code ≠ 0 nur,
wenn alle Regionen scheitern oder die Tabelle leer/unerreichbar ist.
Voraussetzung: die Migration `supabase/migration_v3_multi_region.sql`
wurde einmalig im Supabase SQL-Editor ausgeführt (siehe
[docs/ci.md](docs/ci.md)).

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
`shopping_list`, `uploaded_images`. Die Schema-Version steht in `PRAGMA user_version`; `db::open()`
migriert beim Öffnen automatisch auf die aktuelle Version. Schema-Änderungen
erhöhen `SCHEMA_VERSION` und ergänzen einen Migrationsschritt in `migrate()`
(`src/db.rs`) — bestehende Datenbanken werden dabei in-place aktualisiert.

Das Supabase-Schema (Tabellen `offers`, `regions`, `markets` plus
Storage-Bucket `offer-images`) liegt kanonisch unter
[`supabase/`](supabase/) — `schema.sql` + Migrationen; neue Projekte:
`setup_full.sql`, dann `migration_regions.sql`,
`migration_v3_multi_region.sql`, `migration_v4_region_trigger.sql`,
`migration_v5_image_url.sql` und `migration_v6_storage_bucket.sql` im
SQL-Editor ausführen.

## Preis-Historie

Zusätzlich zur wochenaktuellen `offers`-Tabelle schreibt jeder Push dieselben
Zeilen in die Supabase-Tabelle `price_history` (nicht zu verwechseln mit der
gleichnamigen lokalen SQLite-Tabelle) — als dauerhafter Wochen-Schnappschuss,
damit die App später Preisverläufe anzeigen kann. Upsert-Schlüssel ist
`(market, product, region, valid_from)`: Ein erneuter Push derselben Woche
aktualisiert die Zeilen, statt sie zu duplizieren. Zeilen ohne Preis werden
übersprungen. Fehler beim Historien-Schreiben (z. B. fehlende Tabelle) geben
nur eine Warnung aus — der eigentliche Offers-Push schlägt dadurch nie fehl.

**Manuelle Migration:** `supabase/migration_v7_price_history.sql` einmalig im
Supabase-SQL-Editor ausführen. Bis dahin läuft jeder Push zwar erfolgreich
durch, meldet aber `WARNUNG: Preis-Historie fehlgeschlagen`.

## Dokumentation

- [docs/rewe-cert.md](docs/rewe-cert.md) — Rewe-TLS-Zertifikat einrichten
- [docs/automation.md](docs/automation.md) — nächtlicher launchd-Agent (macOS)
- [docs/cron.md](docs/cron.md) — Cron-Automatisierung und JSON-API
- [docs/ci.md](docs/ci.md) — GitHub-Actions-CI, Nightly-Sync und Migrationen
- [scripts/nightly.sh](scripts/nightly.sh) — Pipeline-Skript sync-regions + watch check (mit Einzel-PLZ-Fallback)
