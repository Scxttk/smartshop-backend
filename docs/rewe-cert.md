# Rewe: TLS-Client-Zertifikat einrichten

Der Rewe-Scraper (`src/scrapers/rewe.rs`) ruft die Mobile-API nicht selbst auf,
sondern shellt zum [`rewerse`-CLI](https://github.com/ByteSizedMarius/rewerse-engineering)
aus. Die Rewe-Mobile-API verlangt mutual TLS: ein Client-Zertifikat samt
privatem Schlüssel, die einmalig aus der offiziellen Rewe-Android-App (APK)
extrahiert werden. Ohne diese beiden Dateien funktioniert **nur** der
Rewe-Scraper nicht — alle anderen Ketten brauchen keine Authentifizierung.

## Was smartshop erwartet

`smartshop fetch --store rewe` prüft vor jedem Aufruf:

1. Das Binary `rewerse` ist im `PATH`. Fehlermeldung sonst:
   `rewerse CLI nicht gefunden — bitte installieren (siehe README)`
2. Zertifikat und Schlüssel existieren. Standardpfade (relativ zum
   Arbeitsverzeichnis): `cert.pem` und `private.key`, überschreibbar mit
   `--cert <pfad>` und `--key <pfad>`. Fehlermeldungen sonst:
   `Zertifikat nicht gefunden: cert.pem` bzw. `Private Key nicht gefunden: private.key`

Hinweis: `rewerse` selbst nennt seinen Standard `certificate.pem` — smartshop
übergibt die Pfade aber immer explizit per `-cert`/`-key`, daher zählt nur, was
du smartshop mitgibst.

## Schritt 1: rewerse installieren

Mit Go-Toolchain:

```sh
go install github.com/ByteSizedMarius/rewerse-engineering/cmd@latest
```

Achte darauf, dass das installierte Binary als `rewerse` im `PATH` liegt
(`$GOPATH/bin` bzw. `~/go/bin` in den `PATH` aufnehmen; bei Bedarf das
Binary in `rewerse` umbenennen). Alternativ ein Release-Binary von der
GitHub-Releases-Seite laden.

## Schritt 2: Rewe-APK besorgen

Lade die aktuelle Rewe-App als APK von einem APK-Mirror (z. B. APKPure).
Laut Upstream-Doku ist die App-Version egal. Bei `.apkx`/`.xapk`-Bundles
zuerst entpacken und die eigentliche `.apk` herausnehmen.

## Schritt 3: PFX aus dem APK extrahieren

Ein APK ist ein ZIP-Archiv:

```sh
unzip -j rewe.apk res/raw/mtls_prod.pfx -d .
```

(Alternativ: `.apk` in `.zip` umbenennen und `res/raw/mtls_prod.pfx`
herauskopieren.)

## Schritt 4: PFX in cert.pem + private.key umwandeln

Das PFX ist mit dem (öffentlich bekannten, im Upstream-Repo dokumentierten)
Passwort `NC3hDTstMX9waPPV` geschützt und verwendet die veraltete
RC2-40-CBC-Verschlüsselung. OpenSSL 3.x braucht deshalb das `-legacy`-Flag:

```sh
openssl pkcs12 -in mtls_prod.pfx -clcerts -nokeys -legacy \
    -passin pass:NC3hDTstMX9waPPV -out cert.pem
openssl pkcs12 -in mtls_prod.pfx -nocerts -nodes -legacy \
    -passin pass:NC3hDTstMX9waPPV -out private.key
```

Bei OpenSSL 1.1 das `-legacy`-Flag weglassen. Windows-Nutzer können
alternativ das PowerShell-Skript `docs/rewerse-engineering.ps1` aus dem
Upstream-Repo verwenden (`./rewerse-engineering.ps1 -PfxPath ./mtls_prod.pfx`);
es nutzt .NET-Krypto statt OpenSSL.

Erwartetes Ergebnis: zwei Textdateien im PEM-Format —

- `cert.pem` mit einem `-----BEGIN CERTIFICATE-----`-Block
- `private.key` mit einem `-----BEGIN PRIVATE KEY-----`-Block

Prüfen:

```sh
openssl x509 -in cert.pem -noout -subject -dates
openssl pkey -in private.key -noout && echo "Key OK"
```

## Schritt 5: Mit smartshop verifizieren

Beide Dateien ins Arbeitsverzeichnis legen (oder Pfade per Flag übergeben)
und einen Trockenlauf starten:

```sh
smartshop fetch --store rewe --zip 50667 --dry-run
# oder mit expliziten Pfaden:
smartshop fetch --store rewe --zip 50667 --cert /pfad/cert.pem --key /pfad/private.key --dry-run
```

Erwartete Ausgabe: Marktsuche, gefundener Markt mit ID, dann die Angebotsliste
(`--dry-run` speichert nichts). Schlägt der Abruf mit
`rewerse markets search fehlgeschlagen` fehl, direkt gegentesten:

```sh
rewerse -cert cert.pem -key private.key markets search -query 50667 -json
```

Liefert das JSON, liegt das Problem bei smartshop; liefert es einen
TLS-Fehler, sind Zertifikat/Schlüssel defekt oder veraltet — dann Extraktion
mit einer aktuellen APK wiederholen.

## Rechtlicher Hinweis

Zertifikat und Schlüssel stammen aus der offiziellen App und identifizieren
die App, nicht dich. Nutze die API maßvoll (die eingebauten smartshop-Abrufe
sind unkritisch) und gib die extrahierten Dateien nicht weiter.
