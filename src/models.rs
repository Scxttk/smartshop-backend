use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Market {
    pub id: String,
    pub name: String,
    /// Filial-Koordinaten, wenn der Store-Finder sie liefert; nationale
    /// Platzhalter und ältere Datenbestände tragen None.
    #[serde(default)]
    pub lat: Option<f64>,
    #[serde(default)]
    pub lon: Option<f64>,
}

impl Market {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Market { id: id.into(), name: name.into(), lat: None, lon: None }
    }

    pub fn with_geo(mut self, lat: Option<f64>, lon: Option<f64>) -> Self {
        self.lat = lat;
        self.lon = lon;
        self
    }
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
