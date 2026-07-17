//! Inline-SVG-Sparklines für den Preisverlauf — kein JavaScript nötig.

const W: f64 = 220.0;
const H: f64 = 44.0;
const PAD: f64 = 4.0;

/// Preisreihe als kleine Sparkline rendern. Bei weniger als zwei Punkten
/// wird nur ein Punkt gezeichnet.
pub fn sparkline(prices: &[f64]) -> String {
    if prices.is_empty() {
        return String::new();
    }
    let min = prices.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = prices.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let span = if (max - min).abs() < f64::EPSILON { 1.0 } else { max - min };

    let x = |i: usize| {
        if prices.len() == 1 {
            W / 2.0
        } else {
            PAD + i as f64 * (W - 2.0 * PAD) / (prices.len() - 1) as f64
        }
    };
    let y = |p: f64| PAD + (max - p) / span * (H - 2.0 * PAD);

    let points: Vec<String> =
        prices.iter().enumerate().map(|(i, p)| format!("{:.1},{:.1}", x(i), y(*p))).collect();
    let last = prices.len() - 1;

    format!(
        "<svg class=\"sparkline\" width=\"{W:.0}\" height=\"{H:.0}\" \
viewBox=\"0 0 {W:.0} {H:.0}\" role=\"img\" aria-label=\"Preisverlauf\">\
<polyline fill=\"none\" stroke=\"#2d6a4f\" stroke-width=\"1.5\" points=\"{}\"/>\
<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"2\" fill=\"#2d6a4f\"/></svg>",
        points.join(" "),
        x(last),
        y(prices[last]),
    )
}
