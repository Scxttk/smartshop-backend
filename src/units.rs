use regex::Regex;
use std::sync::OnceLock;

/// Normalisierte Grundeinheit für Preisvergleiche.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Kilogramm,
    Liter,
}

impl Unit {
    pub fn label(&self) -> &'static str {
        match self {
            Unit::Kilogramm => "kg",
            Unit::Liter => "l",
        }
    }
}

/// Preis pro Grundeinheit (€/kg bzw. €/l).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitPrice {
    pub eur: f64,
    pub unit: Unit,
}

impl UnitPrice {
    pub fn format(&self) -> String {
        format!("{:.2} €/{}", self.eur, self.unit.label())
    }
}

fn re_explicit() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "1 kg = 2.76", "1 l = 0,40" — erste Angabe zählt (bei Ranges die untere)
    RE.get_or_init(|| Regex::new(r"(?i)\b1\s*(kg|l)\s*=\s*(\d+(?:[.,]\d+)?)").unwrap())
}

fn re_multiplied() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "20 x 0,5-l", "4 x 115-g"
    RE.get_or_init(|| {
        Regex::new(r"(?i)(\d+(?:[.,]\d+)?)\s*x\s*(\d+(?:[.,]\d+)?)\s*-?\s*(kg|g|ml|l)\b").unwrap()
    })
}

fn re_quantity() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "250-g-Packg.", "0.75 l", "1,5-l-PET-Fl.", "100 g"
    RE.get_or_init(|| Regex::new(r"(?i)(\d+(?:[.,]\d+)?)\s*-?\s*(kg|g|ml|l)\b").unwrap())
}

fn re_je_unit() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "je kg", "je l" (lose Ware)
    RE.get_or_init(|| Regex::new(r"(?i)\bje\s+(kg|l)\b").unwrap())
}

fn parse_num(s: &str) -> Option<f64> {
    s.replace(',', ".").parse().ok()
}

// (Menge in Grundeinheit, Einheit); g/ml werden nach kg/l umgerechnet
fn to_base(amount: f64, unit: &str) -> Option<(f64, Unit)> {
    match unit.to_lowercase().as_str() {
        "kg" => Some((amount, Unit::Kilogramm)),
        "g" => Some((amount / 1000.0, Unit::Kilogramm)),
        "l" => Some((amount, Unit::Liter)),
        "ml" => Some((amount / 1000.0, Unit::Liter)),
        _ => None,
    }
}

/// Menge aus einem deutschen Mengen-Text, z. B. "je 650-g-Packg." -> 0.65 kg.
pub fn parse_quantity(text: &str) -> Option<(f64, Unit)> {
    if let Some(c) = re_multiplied().captures(text) {
        let count = parse_num(&c[1])?;
        let amount = parse_num(&c[2])?;
        let (base, unit) = to_base(amount, &c[3])?;
        return Some((count * base, unit));
    }
    // Bei Ranges wie "225 - 250-g" matcht nur die Zahl direkt an der Einheit
    if let Some(c) = re_quantity().captures_iter(text).last() {
        let amount = parse_num(&c[1])?;
        return to_base(amount, &c[2]);
    }
    if let Some(c) = re_je_unit().captures(text) {
        return to_base(1.0, &c[1]);
    }
    None
}

/// Explizite Grundpreis-Angabe wie "(1 kg = 2.76)" -> 2.76 €/kg.
pub fn parse_explicit_unit_price(text: &str) -> Option<UnitPrice> {
    let c = re_explicit().captures(text)?;
    let unit = to_base(1.0, &c[1])?.1;
    Some(UnitPrice { eur: parse_num(&c[2])?, unit })
}

/// Grundpreis eines Angebots aus Preis + Textfeldern (subtitle/overline/title)
/// ableiten. Explizite "1 kg = ..."-Angaben haben Vorrang; sonst wird der
/// Angebotspreis durch die geparste Menge geteilt.
pub fn derive_unit_price(price: Option<f64>, texts: &[Option<&str>]) -> Option<UnitPrice> {
    for t in texts.iter().flatten() {
        if let Some(up) = parse_explicit_unit_price(t) {
            return Some(up);
        }
    }
    let price = price?;
    for t in texts.iter().flatten() {
        if let Some((qty, unit)) = parse_quantity(t) {
            if qty > 0.0 {
                return Some(UnitPrice { eur: price / qty, unit });
            }
        }
    }
    None
}

/// Produktnamen für die Gruppierung im Preisvergleich normalisieren:
/// Kleinschreibung, nur alphanumerische Zeichen, Whitespace kollabiert.
pub fn normalize_name(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(s: &str) -> Option<(f64, Unit)> {
        parse_quantity(s)
    }

    #[test]
    fn quantity_simple_gram_package() {
        assert_eq!(q("je 650-g-Packg."), Some((0.65, Unit::Kilogramm)));
        assert_eq!(q("je 250-g-Packg."), Some((0.25, Unit::Kilogramm)));
        assert_eq!(q("je 300-g-Schale"), Some((0.3, Unit::Kilogramm)));
    }

    #[test]
    fn quantity_litre_variants() {
        assert_eq!(q("0.75 l"), Some((0.75, Unit::Liter)));
        assert_eq!(q("je 1,5-l-PET-Fl."), Some((1.5, Unit::Liter)));
        assert_eq!(q("je 2-l-Packg."), Some((2.0, Unit::Liter)));
    }

    #[test]
    fn quantity_multiplied() {
        // Kasten: 20 x 0,5 l = 10 l
        assert_eq!(q("je Ka. 20 x 0,5-l-Fl."), Some((10.0, Unit::Liter)));
        let (v, u) = q("je 4 x 115-g-Becher").unwrap();
        assert!((v - 0.46).abs() < 1e-9);
        assert_eq!(u, Unit::Kilogramm);
    }

    #[test]
    fn quantity_range_takes_value_at_unit() {
        // "225 - 250-g": die an der Einheit hängende Zahl zählt
        assert_eq!(q("je 225 - 250-g-Becher"), Some((0.25, Unit::Kilogramm)));
    }

    #[test]
    fn quantity_loose_goods() {
        assert_eq!(q("je kg"), Some((1.0, Unit::Kilogramm)));
        assert_eq!(q("je 100 g"), Some((0.1, Unit::Kilogramm)));
    }

    #[test]
    fn quantity_unsupported_units() {
        assert_eq!(q("1.2 m²"), None);
        assert_eq!(q("je Stück"), None);
        assert_eq!(q("2 m"), None);
    }

    #[test]
    fn explicit_unit_price() {
        let up = parse_explicit_unit_price("je 650-g-Packg. (1 kg = 2.76)**").unwrap();
        assert_eq!(up.unit, Unit::Kilogramm);
        assert!((up.eur - 2.76).abs() < 1e-9);

        let up = parse_explicit_unit_price("je 1,5-l-PET-Fl. (1 l = 0.40)").unwrap();
        assert_eq!(up.unit, Unit::Liter);
        assert!((up.eur - 0.40).abs() < 1e-9);
    }

    #[test]
    fn explicit_range_takes_lower() {
        let up = parse_explicit_unit_price("je 225 - 250-g-Becher (1 kg = 1.56 - 1.74)").unwrap();
        assert!((up.eur - 1.56).abs() < 1e-9);
    }

    #[test]
    fn explicit_lidl_subtitle_style() {
        // Lidl: subtitle "1 l = 9.98" ohne Klammern
        let up = parse_explicit_unit_price("1 l = 9.98").unwrap();
        assert_eq!(up.unit, Unit::Liter);
        assert!((up.eur - 9.98).abs() < 1e-9);
    }

    #[test]
    fn derive_prefers_explicit() {
        let up = derive_unit_price(
            Some(0.99),
            &[Some("je 500-g-Packg. (1 kg = 1.98)"), None],
        )
        .unwrap();
        assert!((up.eur - 1.98).abs() < 1e-9);
    }

    #[test]
    fn derive_from_quantity() {
        let up = derive_unit_price(Some(3.29), &[Some("0.75 l")]).unwrap();
        assert_eq!(up.unit, Unit::Liter);
        assert!((up.eur - 3.29 / 0.75).abs() < 1e-9);
    }

    #[test]
    fn derive_none_without_price_or_quantity() {
        assert_eq!(derive_unit_price(None, &[Some("je 100 g")]), None);
        assert_eq!(derive_unit_price(Some(1.0), &[Some("je Stück")]), None);
    }

    #[test]
    fn normalize_names() {
        assert_eq!(normalize_name("PARKSIDE® Werkzeugkoffer, 122-tlg."), "parkside werkzeugkoffer 122 tlg");
        assert_eq!(normalize_name("  Cocktail-Rispentomaten "), "cocktail rispentomaten");
    }
}
