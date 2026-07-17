use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Market {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    pub market_id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub overline: Option<String>,
    pub price: Option<f64>,
    pub regular_price: Option<f64>,
    pub category: Option<String>,
    pub nutri_score: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub images: Vec<String>,
    pub biozid: bool,
    pub flyer_page: Option<i64>,
}

impl Offer {
    pub fn build_id(market_id: &str, title: &str, valid_from: Option<&str>) -> String {
        let date = valid_from.unwrap_or("unknown");
        format!("{market_id}_{title}_{date}")
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect()
    }
}
