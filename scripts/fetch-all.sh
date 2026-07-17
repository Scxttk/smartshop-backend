#!/usr/bin/env bash
# fetch-all.sh — cron-tauglicher Wrapper um `smartshop fetch --all-stores`.
#
# Konfiguration über Umgebungsvariablen (alle optional außer SMARTSHOP_ZIP):
#   SMARTSHOP_ZIP   Postleitzahl für die Marktsuche (Pflicht)
#   SMARTSHOP_BIN   Pfad zum smartshop-Binary   (Default: smartshop im PATH)
#   SMARTSHOP_DB    Pfad zur SQLite-Datenbank   (Default: ~/.local/share/smartshop/smartshop.db)
#   SMARTSHOP_LOG   Pfad zur Logdatei           (Default: ~/.local/share/smartshop/fetch-all.log)
#   SMARTSHOP_CERT  Rewe-Zertifikat (PEM)       (Default: cert.pem — siehe docs/rewe-cert.md)
#   SMARTSHOP_KEY   Rewe Private Key            (Default: private.key)
#
# Exit-Code: 0 bei Erfolg, !=0 wenn der Abruf fehlschlägt oder die
# Konfiguration unvollständig ist. Beispiel-Crontab siehe docs/cron.md.

set -euo pipefail

SMARTSHOP_BIN="${SMARTSHOP_BIN:-smartshop}"
SMARTSHOP_DB="${SMARTSHOP_DB:-$HOME/.local/share/smartshop/smartshop.db}"
SMARTSHOP_LOG="${SMARTSHOP_LOG:-$HOME/.local/share/smartshop/fetch-all.log}"
SMARTSHOP_CERT="${SMARTSHOP_CERT:-cert.pem}"
SMARTSHOP_KEY="${SMARTSHOP_KEY:-private.key}"

log() {
    printf '%s %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" >>"$SMARTSHOP_LOG"
}

fail() {
    log "FEHLER: $*"
    echo "fetch-all.sh: $*" >&2
    exit 1
}

mkdir -p "$(dirname "$SMARTSHOP_DB")" "$(dirname "$SMARTSHOP_LOG")"

[ -n "${SMARTSHOP_ZIP:-}" ] || fail "SMARTSHOP_ZIP ist nicht gesetzt (Postleitzahl, z. B. SMARTSHOP_ZIP=50667)."
command -v "$SMARTSHOP_BIN" >/dev/null 2>&1 || fail "smartshop-Binary nicht gefunden: $SMARTSHOP_BIN"

log "Start: fetch --all-stores (PLZ $SMARTSHOP_ZIP, DB $SMARTSHOP_DB)"

# Ausgabe zeilenweise mit Zeitstempel ins Log; Exit-Code von smartshop
# bleibt dank pipefail erhalten.
if "$SMARTSHOP_BIN" fetch --all-stores \
    --zip "$SMARTSHOP_ZIP" \
    --db "$SMARTSHOP_DB" \
    --cert "$SMARTSHOP_CERT" \
    --key "$SMARTSHOP_KEY" \
    --notify 2>&1 | while IFS= read -r line; do log "$line"; done
then
    log "Fertig: Abruf erfolgreich."
else
    rc=$?
    log "FEHLER: smartshop fetch beendet mit Exit-Code $rc."
    exit "$rc"
fi
