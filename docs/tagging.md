# Angebots-Tagging (`match_key`)

Der Push taggt jedes Angebot regelbasiert mit Alltagsbegriffen
(`src/matching.rs` + eingebettetes `docs/matching-woerterbuch.json`) und
schreibt sie in die Spalte `offers.match_key text[]`
(`supabase/migration_v10_match_key.sql`). Werte:

- `{käse}`, `{tomaten}`, … — Begriffs-Tags (auch mehrere möglich)
- `{nonfood}` — erkanntes Non-Food
- `{}` — ungetaggt → Kandidat für die Review-Liste

## Wörterbuch pflegen

Quelle ist `docs/matching-woerterbuch.json` (Sektionen `begriffe` mit
exact/suffix/block und `marken` Marke→Begriff bzw. `NONFOOD`). Nach jeder
Änderung:

1. `python3 docs/matching-woerterbuch-eval.py` — misst Abdeckung gegen die
   lokale Nightly-DB und regeneriert die JSON aus der Python-Referenz.
2. `cargo test matching` und
   `cargo test parity_with_eval_db -- --ignored --nocapture` — die Rust-Zahlen
   müssen exakt zur Python-Ausgabe passen.
3. Neu bauen — die JSON ist per `include_str!` einkompiliert.

## Review-Liste: ungetaggte Angebote der aktuellen KW

Supabase SQL-Editor (oder psql):

```sql
select market, product, valid_from, region
from public.offers
where match_key = '{}'
  and valid_until >= current_date
order by market, product;
```

Per PostgREST/curl:

```sh
curl -s "$SUPABASE_URL/rest/v1/offers?match_key=eq.\{\}&valid_until=gte.$(date +%F)&select=market,product,valid_from,region" \
  -H "apikey: $SUPABASE_SERVICE_KEY" -H "Authorization: Bearer $SUPABASE_SERVICE_KEY"
```

Erwartung: ~2 % der Food-Angebote (kontextlose Flyer-Titel wie „Fix",
„Ciolino"). Neue Begriffe/Marken daraus ins Wörterbuch pflegen (Workflow
oben); geschätzt ~10–30 Einträge/Woche bis zur Konvergenz.
