# CI und Nightly-Lauf auf GitHub Actions

Zwei Workflows liegen unter `.github/workflows/`:

- **`ci.yml`** — läuft bei jedem Push und Pull Request: `cargo check` und
  `cargo test`. Die 7 Live-Scraper-Tests sind `#[ignore]` und werden **nicht**
  ausgeführt (kein Netzwerkzugriff nötig). Rust-/Cargo-Artefakte werden über
  `Swatinem/rust-cache` gecacht.
- **`nightly.yml`** — der Cloud-Ersatz bzw. das Fallback für den lokalen
  launchd-Agenten (`scripts/nightly.sh`): Release-Build, dann
  `fetch --all-stores` in eine temporäre DB, dann `push --region` nach
  Supabase.

## Zeitplan

Der Nightly-Lauf startet per Cron um **04:30 UTC**:

- Sommerzeit (MESZ): **06:30** deutscher Zeit
- Winterzeit (MEZ): **05:30** deutscher Zeit

GitHub-Cron kennt keine Zeitzonen, daher verschiebt sich die lokale Startzeit
mit der Zeitumstellung um eine Stunde. Außerdem garantiert GitHub keine
pünktliche Ausführung — Verzögerungen von einigen Minuten bis über einer
Stunde sind normal.

## Einmalige Einrichtung

Unter **Settings → Secrets and variables → Actions** im Repo:

**Secrets** (Reiter *Secrets*):

| Name | Inhalt |
|---|---|
| `SUPABASE_URL` | Projekt-URL, z. B. `https://xyz.supabase.co` |
| `SUPABASE_SERVICE_KEY` | Service-Role-Key des Supabase-Projekts |

**Variable** (Reiter *Variables*):

| Name | Inhalt |
|---|---|
| `SMARTSHOP_ZIP` | Postleitzahl für die Filialsuche, z. B. `01219` |

Oder per CLI:

```sh
gh secret set SUPABASE_URL
gh secret set SUPABASE_SERVICE_KEY
gh variable set SMARTSHOP_ZIP --body 01219
```

## Manuell starten

Über die GitHub-Oberfläche: **Actions → Nightly fetch+push → Run workflow**.
Oder per CLI:

```sh
gh workflow run nightly.yml
gh run watch          # letzten Lauf live verfolgen
```

## Ergebnis lesen

- **Job-Summary**: Auf der Lauf-Seite (Actions → Lauf anklicken) steht unter
  *Summary* eine Tabelle „Markt / Filiale / Ergebnis" — pro Store entweder die
  Anzahl der Angebote oder `FEHLER: …`, plus die Gesamtzahl der Angebote.
- **Artefakt `nightly-log`**: das vollständige fetch+push-Log, 7 Tage
  aufbewahrt. Auf der Lauf-Seite unter *Artifacts* herunterladen.

## Fehlerverhalten

- **Einzelne Stores dürfen fehlschlagen** — insbesondere REWE, das ohne
  Client-Zertifikat läuft (siehe `docs/rewe-cert.md`) und daher in CI immer
  als `FEHLER` erscheint. Der Job bleibt grün, solange insgesamt Angebote
  ankommen.
- **Der Job schlägt fehl**, wenn der Fetch insgesamt **0 Angebote** liefert
  oder der Push nach Supabase scheitert.

## Bekanntes Risiko: Akamai vs. Runner-IPs

Netto, ALDI Süd und EDEKA werden über system-`curl` geholt, weil Akamai
schlichte reqwest-Clients blockt. GitHub-Runner laufen auf Azure-IP-Ranges,
die Akamai möglicherweise härter blockt als eine private Wohnungs-IP. Wenn
diese Stores **nur auf dem Runner** fehlschlagen (lokal aber laufen), ist das
genau diese IP-Reputation — das ist ein Datenpunkt, kein Bug. In dem Fall
bleibt der lokale launchd-Agent für diese Stores die verlässliche Quelle;
Proxys o. Ä. sind bewusst nicht vorgesehen. Die höfliche Rate-Limitierung der
Scraper gilt unverändert auch in CI.

## Koexistenz mit dem lokalen launchd-Agenten

Beide Pipelines können parallel laufen: der Push nach Supabase ist ein
**Upsert**, doppelte Läufe sind also idempotent und richten keinen Schaden an.
Empfehlung: eine der beiden Seiten abschalten, damit klar ist, welche Quelle
maßgeblich ist —

- **entweder** den launchd-Agenten deaktivieren
  (`scripts/install-launchd.sh --uninstall`, siehe `docs/automation.md`),
- **oder** den `schedule:`-Block in `nightly.yml` auskommentieren und den
  Cloud-Lauf nur manuell (`workflow_dispatch`) nutzen.

Hinweis: der lokale Agent macht zusätzlich `watch check` + ntfy-Benachrichtigung;
das gibt es im Cloud-Lauf (noch) nicht.
