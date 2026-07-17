# smartshop automatisieren: Cron, launchd und die JSON-API

Zwei Bausteine bieten sich für Automatisierung an:

1. `scripts/fetch-all.sh` — holt regelmäßig die Angebote aller Ketten.
2. `smartshop watch check` — meldet Watchlist-Treffer per **Exit-Code 1**
   (0 = keine Treffer), ideal als Cron-Bedingung für Benachrichtigungen.

## Voraussetzungen

- `smartshop` gebaut und im `PATH` (oder `SMARTSHOP_BIN` setzen).
- `curl` im `PATH` (Netto, ALDI Süd und EDEKA nutzen System-curl).
- Für Rewe: `rewerse` + Zertifikat, siehe [rewe-cert.md](rewe-cert.md) —
  ohne läuft `--all-stores` trotzdem durch, Rewe erscheint dann als Fehler
  in der Zusammenfassung.

Hinweis zum Fehlerverhalten: `fetch --all-stores` bricht bei einzelnen
Ketten-Fehlern **nicht** ab und endet mit Exit-Code 0, solange der Lauf als
Ganzes funktioniert. Das Log (`SMARTSHOP_LOG`) enthält die Zusammenfassung
pro Kette.

## Crontab (Linux/macOS)

`crontab -e`, dann z. B. täglicher Abruf um 6:30 und Watchlist-Check um 7:00:

```cron
# Angebote aller Ketten abrufen (Pfade anpassen)
30 6 * * * SMARTSHOP_ZIP=50667 SMARTSHOP_BIN=/usr/local/bin/smartshop /pfad/zu/smartshop/scripts/fetch-all.sh

# Watchlist prüfen: Exit-Code 1 bei Treffern -> Benachrichtigung schicken
0 7 * * * /usr/local/bin/smartshop watch check --db "$HOME/.local/share/smartshop/smartshop.db" > /tmp/smartshop-watch.txt || mail -s "smartshop: neue Deals" ich@example.com < /tmp/smartshop-watch.txt
```

Der Watch-Check nutzt den Exit-Code-Vertrag: `watch check` schreibt die
Treffer nach stdout und beendet sich mit 1, sobald mindestens ein Watch
anschlägt. `||` führt den Notifier also genau dann aus, wenn es Deals gibt.
Statt `mail` funktioniert jeder Notifier, z. B. `notify-send` (Linux-Desktop)
oder ein `curl`-POST an ntfy/Slack:

```cron
0 7 * * * smartshop watch check --db /pfad/smartshop.db > /tmp/w.txt || curl -s -d @/tmp/w.txt ntfy.sh/mein-smartshop-topic
```

Alternativ direkt beim Abruf: `fetch --notify` (in fetch-all.sh bereits
gesetzt) druckt die Watchlist-Treffer ans Ende des Fetch-Logs — ändert aber
nichts am Exit-Code.

## launchd (macOS)

Cron funktioniert auch auf macOS, launchd ist aber der native Weg und holt
verpasste Läufe nach dem Aufwachen nach. Datei
`~/Library/LaunchAgents/de.smartshop.fetch-all.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>de.smartshop.fetch-all</string>
    <key>ProgramArguments</key>
    <array>
        <string>/pfad/zu/smartshop/scripts/fetch-all.sh</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>SMARTSHOP_ZIP</key><string>50667</string>
        <key>SMARTSHOP_BIN</key><string>/usr/local/bin/smartshop</string>
        <key>PATH</key><string>/usr/local/bin:/usr/bin:/bin</string>
    </dict>
    <key>StartCalendarInterval</key>
    <dict>
        <key>Hour</key><integer>6</integer>
        <key>Minute</key><integer>30</integer>
    </dict>
    <key>StandardErrorPath</key>
    <string>/tmp/smartshop-fetch-all.err</string>
</dict>
</plist>
```

Aktivieren / testen / deaktivieren:

```sh
launchctl load ~/Library/LaunchAgents/de.smartshop.fetch-all.plist
launchctl start de.smartshop.fetch-all
launchctl unload ~/Library/LaunchAgents/de.smartshop.fetch-all.plist
```

Für den Watch-Check analog ein zweites plist anlegen, dessen
`ProgramArguments` ein kleines `bash -c`-Kommando mit dem
`watch check || notifier`-Muster enthält.

## JSON-API für Dashboards

`smartshop serve` startet eine **rein lesende** HTTP-API auf Port 8080
(änderbar mit `--port`), z. B. für ein Grafana-/Homepage-Widget oder ein
eigenes Dashboard:

```sh
smartshop serve --db "$HOME/.local/share/smartshop/smartshop.db" --port 8080
```

| Endpoint | Parameter | Liefert |
|---|---|---|
| `GET /markets` | – | Gespeicherte Filialen |
| `GET /offers` | `q` (Pflicht), `max_price`, `market` | Angebote zur Suche |
| `GET /compare` | `q` (Pflicht) | Preisvergleich, gruppiert pro Produkt |
| `GET /stats` | – | Angebote/Filiale + Top-10-Rabatte |
| `GET /history` | `q` (Pflicht) | Preisverlauf |
| `GET /deals` | `since` (Tage, optional) | Preissenkungen |
| `GET /watches` | – | Watchlist-Einträge |
| `GET /watches/check` | – | `{"hits": bool, "watches": [...]}` |
| `GET /list` | – | Einkaufsliste |
| `GET /list/suggest` | – | Günstigster Markt je Listen-Artikel |

Fehler kommen als JSON (`{"error": "..."}`) mit 400 (fehlender/ungültiger
Parameter) bzw. 500. Beispiel für ein Dashboard-Polling:

```sh
curl -s "http://localhost:8080/watches/check" | jq '.hits'
```

Die API bindet an `0.0.0.0` und hat **keine Authentifizierung** — im
Heimnetz betreiben oder per Reverse-Proxy/Firewall absichern. Als Dauerdienst
eignet sich wieder launchd/systemd (Programm: `smartshop serve --db ...`).
