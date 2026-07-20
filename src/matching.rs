//! Regelbasiertes Angebots-Tagging mit Alltagsbegriffen (`match_key`).
//!
//! Port der Python-Referenz `docs/matching-woerterbuch-eval.py` — das
//! Wörterbuch `docs/matching-woerterbuch.json` ist die gemeinsame Quelle
//! und wird zur Compile-Zeit eingebettet. Bei Änderungen an Wörterbuch
//! oder Matching-Regeln IMMER beide Seiten anfassen und den Ignore-Test
//! `parity_with_eval_db` gegen die lokale Nightly-DB laufen lassen.
//!
//! Ergebnis pro Angebot: Liste von Begriffs-Tags ("käse", "tomaten", …),
//! `["nonfood"]` für erkanntes Non-Food, leer für ungetaggt (Review-Liste).

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

pub const NONFOOD_KEY: &str = "nonfood";

const DICT_JSON: &str = include_str!("../docs/matching-woerterbuch.json");

// Ketten-Marketing-Kategorien, die klar Non-Food sind.
const NONFOOD_CAT: &str = r"(?i)mode|style|heim|haus|garten|haustier|tierbedarf|tiernahrung|pflanzen|angeln|elektro|medien|kinderzimmer|wäschepflege|schulstart|kochen-und-grillen|drogerie|spielzeug|alltagshelfer|technik|spielwaren|baumarkt|multimedia|bekleidung|schuhe|camping|auto|buero|non.?food";

// Non-Food-Begriffe im Titel (fängt Non-Food in Food-Kategorien wie „Wochenangebote").
const NONFOOD_TERMS: &str = r"(?i)lichterkette|lampion|wäschest|wäscheklammer|wäschekorb|kettensäge|akku|werkzeug|kinderbuch|spielzeug|rosen\b|blumen|pflanze|socken|shorts|shirt|cap\b|hose|schuhe|handtuch|bettwäsche|pfannen?\b|topf\b|löffel|messer|grill\b|kohle|batterie|lampe|leuchte|katzen|hunde|tiernahrung|nassfutter|trockenfutter|snack für|rasenkanten|solar|deko|kissen|matratze|drucker|kopfhörer|wc-|reiniger|megaperls|oxi action|schreibwaren|mikrofon|duschregal|sonnensegel|wäscheparf|karaoke|trinkzubehör|wäschetrockner|weißer riese|sonnenspray|duftspüler|sonnencreme|feuchttücher|servietten|haushaltstücher|klumpstreu|geschirrtücher|platzset|schlafsack|fusselrolle|bügeleisen|glasschüssel|lautsprecher|geräusche-box|fliegengitter|kajak|husarenknöpfchen|lavendel|bilderbuch|wecker|hairstyler|bastelkoffer|kochgeschirr|grillplatte|boombox|fliegenfalle|mottenabwehr|badvorleger|schrubber|kosmetikspiegel|shorty|plaid|fototafel|komfort-bh|pantoletten|spannbetttuch|küchentücher|sneaker|hoodie|bodyspray|deospray|sonnenschutz|dutch oven|gläsersortiment|sonnenschirm|tischdecke|fleece|wellnessbürste|maniküre|pediküre|teppich|taillenslip|haftcreme|wasserballon|corega|axe ";

// Tokens, bei denen Suffix-Matching generell verboten ist (falsche Komposita).
const SUFFIX_STOP: &[&str] = &[
    "reis", "preis", "schwein", "schweine", "kreis", "eis", "wein", "hackfleisch", "gehacktes",
    "abwaschbecken",
];

struct Term {
    key: String,
    /// Einwort-Begriffe: Token-Gleichheit. Mehrwort-Begriffe: Substring in ntext.
    exact: Vec<String>,
    /// Komposita-Suffixe (nur ab 4 Zeichen wirksam).
    suffix: Vec<String>,
    block_words: Vec<String>,
    block_phrases: Vec<String>,
}

struct Dict {
    terms: Vec<Term>,
    /// Marke (normalisiert) → Begriff bzw. NONFOOD_KEY; Reihenfolge = JSON-Reihenfolge.
    brands: Vec<(String, String)>,
    nonfood_cat: Regex,
    nonfood_terms: Regex,
}

fn dict() -> &'static Dict {
    static DICT: OnceLock<Dict> = OnceLock::new();
    DICT.get_or_init(|| {
        let v: serde_json::Value =
            serde_json::from_str(DICT_JSON).expect("matching-woerterbuch.json ungültig");
        let terms = v["begriffe"]
            .as_object()
            .expect("Sektion 'begriffe' fehlt")
            .iter()
            .map(|(key, def)| {
                let list = |field: &str| -> Vec<String> {
                    def[field]
                        .as_array()
                        .map(|a| {
                            a.iter().filter_map(|s| s.as_str()).map(norm).collect()
                        })
                        .unwrap_or_default()
                };
                let (block_phrases, block_words) =
                    list("block").into_iter().partition(|b| b.contains(' '));
                Term {
                    key: key.clone(),
                    exact: list("exact"),
                    suffix: list("suffix").into_iter().filter(|s| s.chars().count() >= 4).collect(),
                    block_words,
                    block_phrases,
                }
            })
            .collect();
        let brands = v["marken"]
            .as_object()
            .expect("Sektion 'marken' fehlt")
            .iter()
            .filter_map(|(brand, term)| {
                let b = norm(brand);
                let t = term.as_str()?;
                if b.is_empty() {
                    return None;
                }
                let key = if t == "NONFOOD" { NONFOOD_KEY.to_string() } else { t.to_string() };
                Some((b, key))
            })
            .collect();
        Dict {
            terms,
            brands,
            nonfood_cat: Regex::new(NONFOOD_CAT).unwrap(),
            nonfood_terms: Regex::new(NONFOOD_TERMS).unwrap(),
        }
    })
}

/// Normalisierung wie in der Python-Referenz: lowercase, ®*™ raus,
/// Bindestrich = Leerzeichen, Akzente flachziehen (Chicorée), alles außer
/// a-zäöüß zu Leerzeichen, Whitespace kollabieren.
fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.to_lowercase().chars() {
        match c {
            '®' | '*' | '™' => {}
            '-' => out.push(' '),
            'é' | 'è' | 'ê' => out.push('e'),
            'á' | 'à' | 'â' => out.push('a'),
            'í' | 'ì' => out.push('i'),
            'ó' | 'ò' => out.push('o'),
            'ú' | 'ù' => out.push('u'),
            'a'..='z' | 'ä' | 'ö' | 'ü' | 'ß' | ' ' => out.push(c),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Tokens ab 3 Zeichen, plus Plural-Varianten ohne Endungs-s/-n/-e.
fn tokens(ntext: &str) -> Vec<String> {
    let base: Vec<String> = ntext.split(' ').filter(|t| t.chars().count() > 2).map(String::from).collect();
    let mut all = base.clone();
    for t in &base {
        if t.chars().count() > 4 {
            if let Some(last) = t.chars().last() {
                if matches!(last, 's' | 'n' | 'e') {
                    all.push(t[..t.len() - last.len_utf8()].to_string());
                }
            }
        }
    }
    all
}

/// `match_key`-Tags für ein Angebot: Begriffs-Tags, `["nonfood"]` für
/// erkanntes Non-Food, leer für ungetaggt.
pub fn match_keys(title: &str, subtitle: Option<&str>, category: Option<&str>) -> Vec<String> {
    let d = dict();
    let text = match subtitle {
        Some(sub) if !sub.is_empty() => format!("{title} {sub}"),
        _ => title.to_string(),
    };
    if d.nonfood_cat.is_match(category.unwrap_or("")) || d.nonfood_terms.is_match(&text) {
        return vec![NONFOOD_KEY.to_string()];
    }
    let ntext = norm(&text);
    let toks: HashSet<String> = tokens(&ntext).into_iter().collect();

    let mut hits: Vec<String> = Vec::new();
    for term in &d.terms {
        if term.block_phrases.iter().any(|b| ntext.contains(b.as_str()))
            || term.block_words.iter().any(|b| toks.contains(b))
        {
            continue;
        }
        let exact_hit = term
            .exact
            .iter()
            .any(|e| toks.contains(e) || (e.contains(' ') && ntext.contains(e.as_str())));
        let suffix_hit = || {
            term.suffix.iter().any(|sfx| {
                toks.iter().any(|t| {
                    t.ends_with(sfx.as_str())
                        && !SUFFIX_STOP.contains(&t.as_str())
                        && !term.block_words.contains(t)
                })
            })
        };
        if exact_hit || suffix_hit() {
            hits.push(term.key.clone());
        }
    }
    if hits.is_empty() {
        // Marken-Fallback: erste passende Marke gewinnt (JSON-Reihenfolge).
        for (brand, key) in &d.brands {
            if ntext.contains(brand.as_str()) {
                return vec![key.clone()];
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(title: &str) -> Vec<String> {
        match_keys(title, None, None)
    }

    #[test]
    fn regressionsfaelle() {
        assert_eq!(keys("Nadler Edle Matjesfilets"), vec!["fisch"]);
        assert_eq!(keys("Tomatenmark"), vec!["konserven"]);
        let ts = keys("Thunfisch-Salat");
        assert!(ts.contains(&"fisch".to_string()) && !ts.contains(&"salat".to_string()), "{ts:?}");
        assert!(keys("Kirschtomaten").contains(&"tomaten".to_string()));
        assert!(keys("Milka Schokolade").contains(&"schokolade".to_string()));
        assert!(keys("Chicorée").contains(&"brokkoli".to_string()));
        assert!(keys("Mini-Pak-Choi").contains(&"obst".to_string()));
        // Aus der Feedback-Schleife (docs/feedback-auswertung.md): „Käse“ traf
        // ein Schinken-Käse-Croissant. `croissant` steht seither auf der
        // Blockliste — echter Käse darf davon nichts merken.
        let croissant = keys("Schinken-Käse-Croissant");
        assert!(!croissant.contains(&"käse".to_string()), "{croissant:?}");
        assert!(keys("Gouda jung 48% Fett i. Tr.").contains(&"käse".to_string()));
    }

    #[test]
    fn marken_fallback() {
        assert_eq!(keys("Fruchtzwerge"), vec!["joghurt"]);
        assert_eq!(keys("Bitburger Premium"), vec!["bier"]);
    }

    #[test]
    fn nonfood_und_ungetaggt() {
        assert_eq!(match_keys("Duschbad", None, Some("drogerie")), vec![NONFOOD_KEY]);
        assert_eq!(keys("Sagrotan Hygiene-Spray 2in1"), vec!["windeln/hygiene"]);
        assert_eq!(keys("Crivit Trekkingstöcke"), vec![NONFOOD_KEY]);
        assert!(keys("Ciolino").is_empty()); // kontextloser Flyer-Titel → Review-Liste
    }

    /// Paritäts-Check gegen die Python-Referenz auf der lokalen Nightly-DB:
    /// `cargo test parity_with_eval_db -- --ignored --nocapture`
    /// Die Zahlen müssen zur Ausgabe von `python3 docs/matching-woerterbuch-eval.py` passen.
    #[test]
    #[ignore]
    fn parity_with_eval_db() {
        let path = std::env::var("HOME").unwrap() + "/.local/share/smartshop/smartshop.db";
        let conn = rusqlite::Connection::open(&path).unwrap();
        let mut stmt = conn
            .prepare(
                "select o.title, coalesce(o.subtitle,''), coalesce(o.category,'')
                 from offers o join markets m on m.id=o.market_id
                 where o.valid_until >= date('now')",
            )
            .unwrap();
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let (mut nonfood, mut tagged, mut untagged) = (0, 0, 0);
        for (title, sub, cat) in &rows {
            let k = match_keys(title, Some(sub), Some(cat));
            if k == [NONFOOD_KEY] {
                nonfood += 1;
            } else if k.is_empty() {
                untagged += 1;
            } else {
                tagged += 1;
            }
        }
        println!(
            "gesamt {} | nonfood {} | getaggt {} | ungetaggt {}",
            rows.len(),
            nonfood,
            tagged,
            untagged
        );
    }
}
