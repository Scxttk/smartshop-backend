#!/usr/bin/env python3
"""Aus abgelehnten Treffern (`match_feedback`) Wörterbuch-Vorschläge machen.

Kein Machine Learning. Dafür fehlt die Datenmenge — die App hat einen Nutzer,
ein Modell bräuchte Tausende gelabelte Beispiele. Es braucht auch keines: das
Wörterbuch ist eine Regelstruktur, und jede Ablehnung zeigt auf genau eine
Regel. Die Auswahloption im Sheet sagt bereits, *welche* Operation gemeint ist:

    wrong_product   -> Wort auf die block-Liste des verantwortlichen Begriffs
    wrong_variant   -> block, oder ein neuer feinerer Begriff mit eigenem exact
    wrong_size      -> KEINE Wörterbuchänderung (Mengenproblem, siehe Backlog)
    personal_taste  -> KEINE Änderung. Zählen, nicht einarbeiten
    other           -> Freitext lesen

Dieses Skript holt die Zeilen, wirft weg was keine Regel betrifft, bestimmt den
verantwortlichen Wörterbuch-Eintrag und stellt pro Fall genau eine eng
umrissene Frage. Es ändert selbst nichts: Ausgabe ist ein Vorschlag, kein
Commit. Der Schiedsrichter ist danach das Eval-Skript, nicht dieses hier —
siehe docs/feedback-auswertung.md.

Aufrufe:
    python3 docs/feedback-auswertung.py                  # Supabase, letzte 7 Tage
    python3 docs/feedback-auswertung.py --tage 30
    python3 docs/feedback-auswertung.py --datei dump.json
    python3 docs/feedback-auswertung.py --demo           # eingebaute Beispielzeilen
    python3 docs/feedback-auswertung.py --json faelle.json

Umgebung (wie docs/tagging.md): SUPABASE_URL, SUPABASE_SERVICE_KEY.
Der anon key reicht nicht — die Tabelle hat bewusst keine Select-Policy.
"""
import argparse
import datetime
import importlib.util
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from collections import Counter, defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))

# Wörterbuch + Trefferregel aus dem Eval-Skript, nicht nachgebaut: ein Vorschlag,
# der am falschen Eintrag ansetzt, ist schlimmer als kein Vorschlag.
_spec = importlib.util.spec_from_file_location(
    "woerterbuch_eval", os.path.join(HERE, "matching-woerterbuch-eval.py")
)
ev = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ev)

# Gründe, die eine Wörterbuchänderung rechtfertigen können.
ACTIONABLE = {"wrong_product", "wrong_variant", "other"}
# Gründe, die nur gezählt werden. `personal_taste` ist der wichtige Fall: das
# ist die Vorliebe einer Person, und sie ins Wörterbuch zu tragen würde die
# Daten für alle anderen vergiften.
COUNT_ONLY = {"wrong_size", "personal_taste"}

DEMO_ROWS = [
    # Klassiker aus dem Laufplan: „Käse" trifft ein Schinken-Käse-Croissant.
    {"query": "Käse", "product_title": "Schinken-Käse-Croissant", "market": "Lidl",
     "match_kind": "category", "reason": "wrong_product", "comment": "das ist ein Brötchen"},
    {"query": "käse", "product_title": "Schinken-Käse-Croissant", "market": "Netto",
     "match_kind": "category", "reason": "wrong_product", "comment": None},
    {"query": "Käse", "product_title": "Käsekuchen", "market": "Kaufland",
     "match_kind": "category", "reason": "wrong_product", "comment": None},
    {"query": "Milch", "product_title": "Milchschnitte", "market": "Lidl",
     "match_kind": "category", "reason": "wrong_product", "comment": "Süßigkeit"},
    {"query": "Wurst", "product_title": "Wiener Würstchen", "market": "Netto",
     "match_kind": "category", "reason": "wrong_variant", "comment": "ich wollte Aufschnitt"},
    {"query": "Tomaten", "product_title": "Rispentomaten", "market": "REWE",
     "match_kind": "category", "reason": "wrong_size", "comment": "nur 250 g"},
    {"query": "Bier", "product_title": "Radler naturtrüb", "market": "Lidl",
     "match_kind": "category", "reason": "personal_taste", "comment": None},
    {"query": "Kaffee", "product_title": "Kaffeesahne", "market": "Kaufland",
     "match_kind": "category", "reason": "personal_taste", "comment": "mag ich nicht"},
    {"query": "Nudeln", "product_title": "Nudelsalat mit Ei", "market": "Netto",
     "match_kind": "category", "reason": "wrong_product", "comment": None},
    {"query": "Vollmilch", "product_title": "Bio Vollmilch 3,8 %", "market": "Lidl",
     "match_kind": "direct", "reason": "other", "comment": "falsche Filiale"},
]


def fetch_supabase(days):
    url = os.environ.get("SUPABASE_URL")
    key = os.environ.get("SUPABASE_SERVICE_KEY")
    if not url or not key:
        sys.exit("SUPABASE_URL und SUPABASE_SERVICE_KEY müssen gesetzt sein "
                 "(oder --datei / --demo benutzen).")
    since = (datetime.date.today() - datetime.timedelta(days=days)).isoformat()
    query = urllib.parse.urlencode({
        "select": "query,product_title,market,match_kind,reason,comment,created_at",
        "created_at": f"gte.{since}",
        "order": "created_at.desc",
    })
    request = urllib.request.Request(
        f"{url.rstrip('/')}/rest/v1/match_feedback?{query}",
        headers={"apikey": key, "Authorization": f"Bearer {key}"},
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        sys.exit(f"Supabase antwortete {error.code}: {error.read().decode()[:200]}")


def responsible_terms(row):
    """Wörterbuch-Einträge, die diesen Treffer erzeugt haben können.

    Ein Kategorie-Treffer entsteht über einen Begriff, den sowohl die Suche als
    auch das Produkt trifft — genau dessen exact/suffix/block ist die Stellschraube.
    Ein Direkttreffer entsteht ohne Wörterbuch: das Suchwort stand wörtlich im
    Titel. Dafür gibt es keinen Eintrag zu reparieren, und der Fall gehört
    getrennt betrachtet.
    """
    if row.get("match_kind") == "direct":
        return []
    from_query = set(ev.term_hits(row["query"]))
    from_title = set(ev.term_hits(row["product_title"]))
    return sorted(from_query & from_title)


def block_candidates(term, product_title):
    """Tokens des Produkttitels, die noch auf keiner Liste des Begriffs stehen.

    Bewusst nur Kandidaten, keine Entscheidung: welches Token das Produkt vom
    gesuchten Artikel unterscheidet, ohne echte Treffer zu verlieren, ist genau
    die Frage, die ein Mensch oder ein LLM pro Fall beantwortet.
    """
    exact, suffixes, block = ev.V.get(term, ([], [], []))
    known = {ev.norm(x) for x in list(exact) + list(suffixes) + list(block)}
    seen, candidates = set(), []
    # Nur echte Titelwörter. `ev.tokens` hängt zusätzlich Stammformen ohne
    # End-s/-n/-e an ("schinke", "würstche"); auf einer Blockliste wäre das
    # Unsinn, und als Vorschlag ist es nur Rauschen.
    for token in ev.norm(product_title).split():
        if token in known or token in seen or len(token) < 4:
            continue
        seen.add(token)
        candidates.append(token)
    return candidates


def group_cases(rows):
    """Fasst Zeilen zu (Suchbegriff, Produkt) zusammen — die Einheit, über die
    entschieden wird. Zehn Meldungen zum selben Fehltreffer sind ein Fall, kein
    zehnfaches Gewicht."""
    grouped = defaultdict(lambda: {"count": 0, "reasons": Counter(),
                                   "comments": [], "markets": set(), "kinds": set()})
    for row in rows:
        key = (row["query"].strip().lower(), row["product_title"])
        case = grouped[key]
        case["count"] += 1
        case["reasons"][row["reason"]] += 1
        case["markets"].add(row.get("market") or "?")
        case["kinds"].add(row.get("match_kind") or "?")
        if row.get("comment"):
            case["comments"].append(row["comment"])
    return grouped


def build_cases(rows):
    actionable, counted = [], []
    for (query, product), data in group_cases(rows).items():
        reasons = data["reasons"]
        case = {
            "query": query,
            "product_title": product,
            "markets": sorted(data["markets"]),
            "match_kinds": sorted(data["kinds"]),
            "count": data["count"],
            "reasons": dict(reasons),
            "comments": data["comments"],
        }
        if not (set(reasons) & ACTIONABLE):
            counted.append(case)
            continue
        # Nur die handlungsrelevanten Meldungen bestimmen den Vorschlag; ein
        # „mag ich nicht" in derselben Gruppe darf ihn nicht mitbegründen.
        case["actionable_count"] = sum(n for r, n in reasons.items() if r in ACTIONABLE)
        terms = responsible_terms({"query": query, "product_title": product,
                                   "match_kind": sorted(data["kinds"])[0]})
        case["terms"] = terms
        case["candidates"] = {t: block_candidates(t, product) for t in terms}
        actionable.append(case)
    actionable.sort(key=lambda c: -c["actionable_count"])
    counted.sort(key=lambda c: -c["count"])
    return actionable, counted


def print_report(rows, actionable, counted):
    total = len(rows)
    by_reason = Counter(r["reason"] for r in rows)
    print(f"Rückmeldungen: {total}")
    for reason, count in by_reason.most_common():
        mark = "→ Wörterbuch" if reason in ACTIONABLE else "  nur zählen"
        print(f"  {reason:16s} {count:4d}   {mark}")
    print(f"\nFälle mit möglicher Wörterbuchänderung: {len(actionable)}")
    print(f"Fälle ohne (Menge/Vorliebe):            {len(counted)}")

    if counted:
        print("\n== Nur gezählt (keine Änderung) ==")
        for case in counted:
            reasons = ", ".join(f"{r}×{n}" for r, n in case["reasons"].items())
            print(f"  {case['count']:3d}× „{case['query']}“ → {case['product_title'][:50]}  ({reasons})")

    print("\n== Vorschlagsfragen ==")
    if not actionable:
        print("  (nichts zu tun)")
    for index, case in enumerate(actionable, 1):
        print(f"\n--- Fall {index} ---")
        print(f"Suchbegriff : {case['query']}")
        print(f"Produkt     : {case['product_title']}")
        print(f"Markt/Art   : {', '.join(case['markets'])} / {', '.join(case['match_kinds'])}")
        reasons = ", ".join(f"{r}×{n}" for r, n in case["reasons"].items())
        print(f"Meldungen   : {case['count']} ({reasons})")
        for comment in case["comments"][:3]:
            print(f"Kommentar   : {comment[:120]}")

        if "direct" in case["match_kinds"]:
            print("Eintrag     : keiner — Direkttreffer, das Suchwort stand wörtlich im Titel.")
            print("Frage       : Ist der Suchbegriff mehrdeutig? Wenn ja, braucht es einen")
            print("              eigenen, feineren Begriff — eine Blockliste hilft hier nicht.")
            continue
        if not case["terms"]:
            print("Eintrag     : keiner — das heutige Wörterbuch erzeugt diesen Treffer")
            print("              nicht mehr. Entweder ist er schon behoben, oder er kam")
            print("              über die Markenliste. Nichts zu tun, außer nachsehen.")
            continue

        for term in case["terms"]:
            exact, suffixes, block = ev.V[term]
            print(f"Eintrag     : {term}")
            print(f"  exact  : {exact}")
            print(f"  suffix : {suffixes}")
            print(f"  block  : {block}")
            candidates = case["candidates"][term]
            print(f"  Kandidaten für block: {candidates}")
            print(f'  Frage: Suchbegriff „{case["query"]}“, Produkt „{case["product_title"]}“ —')
            if candidates:
                print(f"         welches Token gehört auf die Blockliste von „{term}“,")
                print(f"         ohne echte {term}-Treffer zu verlieren?")
            else:
                # Jedes Wort des Titels steht schon auf einer Liste des Begriffs:
                # das Produkt *ist* ein Vertreter, nur nicht der gewünschte.
                # Eine Blockliste würde hier echte Treffer mitreißen.
                print(f"         alle Titelwörter gehören bereits zu „{term}“. Blocken würde")
                print(f"         echte Treffer kosten — braucht es einen feineren Begriff")
                print(f"         mit eigenem exact, oder ist die Meldung eine Vorliebe?")

    print("\nAusgabe ist ein Vorschlag, kein Commit. Übernahme und Prüfung:")
    print("  docs/feedback-auswertung.md")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--tage", type=int, default=7,
                        help="Zeitraum in Tagen (Standard 7)")
    parser.add_argument("--datei", help="JSON-Dump statt Supabase lesen")
    parser.add_argument("--demo", action="store_true",
                        help="eingebaute Beispielzeilen statt echter Daten")
    parser.add_argument("--json", dest="json_out",
                        help="Fälle zusätzlich als JSON hierhin schreiben")
    args = parser.parse_args()

    if args.demo:
        rows = DEMO_ROWS
    elif args.datei:
        with open(args.datei, encoding="utf-8") as handle:
            rows = json.load(handle)
    else:
        rows = fetch_supabase(args.tage)

    if not rows:
        print("Keine Rückmeldungen im Zeitraum.")
        return

    actionable, counted = build_cases(rows)
    print_report(rows, actionable, counted)

    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as handle:
            json.dump({"actionable": actionable, "counted": counted},
                      handle, ensure_ascii=False, indent=1)
        print(f"\nFälle geschrieben: {args.json_out}")


if __name__ == "__main__":
    main()
