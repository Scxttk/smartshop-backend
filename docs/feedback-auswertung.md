# Von der Rückmeldung zum Wörterbuch-Eintrag

Die App fragt nach, wenn jemand einen Treffer weglegt, und schreibt die Antwort
nach `match_feedback` (Tabelle: `supabase/migration_match_feedback.sql` im
App-Repo). Dieses Dokument beschreibt, wie daraus ein Wörterbuch-Eintrag wird.

Es ist ein **wöchentlicher Stapellauf**, kein Automatismus. Nichts hier ändert
das Wörterbuch von selbst; die letzte Instanz ist immer das Eval-Skript plus ein
Mensch.

## Warum kein Machine Learning

Die App hat einen Nutzer. Ein Modell bräuchte Tausende gelabelte Beispiele. Es
braucht auch keines: das Wörterbuch ist eine Regelstruktur
(`begriff -> (exact, suffix, block)`), und jede Ablehnung zeigt auf genau eine
Regel. Die Auswahloption im Sheet sagt bereits, welche Operation gemeint ist:

| Antwort im Sheet | `reason` | Wörterbuch-Operation |
|---|---|---|
| Passt gar nicht zum Artikel | `wrong_product` | Wort auf die `block`-Liste des Begriffs |
| Falsche Sorte oder Variante | `wrong_variant` | `block`, oder neuer feinerer Begriff mit eigenem `exact` |
| Falsche Menge oder Größe | `wrong_size` | **keine** — Mengenproblem, siehe Backlog |
| Mag ich einfach nicht | `personal_taste` | **keine** — Vorliebe. Zählen, nicht einarbeiten |
| Anderes | `other` | Freitext lesen |

Der Wert steckt in der Option, nicht im Freitext. `personal_taste` ins
Wörterbuch zu tragen würde die Daten für alle anderen vergiften — genau deshalb
fragt das Sheet danach.

## Ablauf

### 1. Fälle holen

```sh
export SUPABASE_URL=... SUPABASE_SERVICE_KEY=...   # anon key reicht nicht:
                                                   # die Tabelle hat bewusst
                                                   # keine Select-Policy
python3 docs/feedback-auswertung.py --tage 7
```

Das Skript gruppiert nach (Suchbegriff, Produkttitel) — zehn Meldungen zum
selben Fehltreffer sind *ein* Fall, kein zehnfaches Gewicht —, wirft alles ohne
Wörterbuchbezug raus und bestimmt pro Fall den verantwortlichen Eintrag. Dafür
importiert es `V` und die Trefferregel aus `matching-woerterbuch-eval.py`, statt
sie nachzubauen: ein Vorschlag, der am falschen Eintrag ansetzt, ist schlimmer
als kein Vorschlag.

Ohne Zugang zur Datenbank:

```sh
python3 docs/feedback-auswertung.py --demo      # eingebaute Beispielzeilen
python3 docs/feedback-auswertung.py --datei dump.json
```

### 2. Pro Fall genau eine Frage beantworten

Das Skript stellt sie schon formuliert, samt aktuellem Eintrag und den Tokens,
die noch auf keiner seiner Listen stehen:

> Suchbegriff „käse“, Produkt „Schinken-Käse-Croissant“ — welches Token gehört
> auf die Blockliste von „käse“, ohne echte käse-Treffer zu verlieren?

Das kann ein Mensch beantworten oder ein LLM. Wichtig ist der Zuschnitt: **eine**
Frage, **ein** Eintrag, Ausgabe ist ein Diff gegen `V`, kein fertiger Commit.

Drei Fälle bekommen keine Blocklisten-Frage:

- **Direkttreffer** — das Suchwort stand wörtlich im Titel, es gibt keinen
  Eintrag zu reparieren. Wenn überhaupt, braucht der Begriff eine feinere
  Aufteilung.
- **Kein Eintrag trifft mehr** — das heutige Wörterbuch erzeugt diesen Treffer
  gar nicht mehr. Schon behoben, oder er kam über die Markenliste.
- **Alle Titelwörter gehören zum Begriff** — das Produkt *ist* ein Vertreter,
  nur nicht der gewünschte. Blocken würde echte Treffer mitreißen; hier hilft
  nur ein feinerer Begriff mit eigenem `exact` — oder es war doch eine Vorliebe.

### 3. Übernehmen

Der Vorschlag wandert von Hand in `V` in `docs/matching-woerterbuch-eval.py`.
Die Python-Referenz ist die Quelle; `matching-woerterbuch.json` wird daraus
erzeugt, nicht umgekehrt.

### 4. Das Eval-Skript ist der Schiedsrichter

Ein Vorschlag wird nur übernommen, wenn die Abdeckung **nicht fällt** und die
Regressionsfälle weiter stimmen. Damit kann ein Fix für „Käse“ nicht heimlich
„Butter“ zerschießen.

```sh
python3 docs/matching-woerterbuch-eval.py    # Abdeckung + regeneriert die JSON
cargo test matching                          # Regressionsfälle (src/matching.rs)
cargo test parity_with_eval_db -- --ignored --nocapture   # Rust == Python
```

Vorher/nachher vergleichen: die Zeile `regelbasiert getaggt: N (X%)`. Fällt sie,
ist der Vorschlag zu grob — meist wurde ein Token geblockt, das auch in echten
Treffern vorkommt.

Danach neu bauen: die JSON ist per `include_str!` einkompiliert (siehe
`docs/tagging.md`).

### 5. Gegenprobe

Der konkret gemeldete Fehltreffer muss weg sein. Am schnellsten über das
Eval-Skript selbst, das jeden Treffer pro Begriff auflistet, oder über einen
Regressionsfall in `src/matching.rs` — für einen Fall, der einmal echt gemeldet
wurde, lohnt sich die Zeile.
