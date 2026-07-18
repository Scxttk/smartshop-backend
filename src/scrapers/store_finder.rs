//! Filial-Lookup für die Ketten mit nationalem Angebotskatalog (Lidl,
//! ALDI Nord, ALDI SÜD). Deren Angebots-APIs sind bundesweit — ob die Kette
//! in einer Region überhaupt vertreten ist, klären die offiziellen
//! Store-Finder der Ketten:
//!
//!   Lidl:      Bing Spatial Data Service (spatial.virtualearth.net), Dataset
//!              Filialdaten-SEC — dieselbe Quelle nutzt der Filialfinder auf
//!              lidl.de. `spatialFilter=nearby(lat,lon,km)` filtert serverseitig.
//!   ALDI Nord/SÜD: Uberall-Locator (uberall.com/api/storefinders/<key>),
//!              die Plattform hinter den Filialfindern auf aldi-nord.de bzw.
//!              aldi-sued.de. Sucht per lat/lng, liefert `distance` in Metern.
//!
//! Beide brauchen Koordinaten statt PLZ; die PLZ geocodiert Nominatim
//! (nominatim.openstreetmap.org, 1 Request pro Region, mit eigenem UA laut
//! OSM-Policy). Alle drei Hosts sind nicht Akamai-geschützt — reqwest mit dem
//! gemeinsamen Browser-UA (util::blocking_client) reicht, anders als bei den
//! Angebots-Scrapern von Netto/ALDI SÜD (System-curl).
//!
//! Fehlerverhalten: Finder-Fehler (Netz, Formatänderung) fallen mit WARN auf
//! den nationalen Platzhalter zurück — lieber ein zu breiter Eintrag als eine
//! stumm verschwundene Kette. Nur ein *erfolgreicher* Lookup ohne Filiale im
//! Umkreis meldet die Kette als nicht vertreten.

use anyhow::{Context, Result, bail};

use crate::models::Market;
use crate::scrapers::util;

/// Maximale Entfernung Filiale <-> PLZ-Zentrum, ab der die Kette als in der
/// Region vertreten gilt.
pub const CUTOFF_KM: f64 = 15.0;

// Öffentliche Keys der Filialfinder (in den Websites der Ketten eingebettet,
// Stand 2026-07; Quelle: alltheplaces-Spiders lidl_de / aldi_nord_de / aldi_sud_de).
const LIDL_DATASET: &str = "ab055fcbaac04ec4bc563e65ffa07097";
const LIDL_KEY: &str = "AnTPGpOQpGHsC_ryx9LY3fRTI27dwcRWuPrfg93-WZR2m-1ax9e9ghlD4s1RaHOq";
const ALDI_NORD_KEY: &str = "ALDINORDDE_UimhY3MWJaxhjK9QdZo3Qa4chq1MAu";
const ALDI_SUED_KEY: &str = "gqNws2nRfBBlQJS9UrA8zV9txngvET";

// ---------------------------------------------------------------- Geocoding

/// PLZ -> Koordinaten über Nominatim (OSM). Ein Request pro Region.
pub fn geocode_plz(plz: &str) -> Result<(f64, f64)> {
    let url = format!(
        "https://nominatim.openstreetmap.org/search?postalcode={plz}&country=de&format=jsonv2&limit=1"
    );
    util::polite_pause(&url);
    let raw: serde_json::Value = util::blocking_client()?
        .get(&url)
        .send()
        .with_context(|| util::ctx("Store-Finder", "PLZ geocodieren", &url))?
        .error_for_status()
        .with_context(|| util::ctx("Store-Finder", "PLZ geocodieren (HTTP-Status)", &url))?
        .json()
        .with_context(|| util::ctx("Store-Finder", "Geocoding JSON parsen", &url))?;
    parse_nominatim(&raw).with_context(|| format!("Nominatim kennt PLZ {plz} nicht"))
}

/// Erstes Ergebnis einer Nominatim-Antwort als (lat, lon).
pub fn parse_nominatim(raw: &serde_json::Value) -> Option<(f64, f64)> {
    let first = raw.as_array()?.first()?;
    let lat = first.get("lat")?.as_str()?.parse().ok()?;
    let lon = first.get("lon")?.as_str()?.parse().ok()?;
    Some((lat, lon))
}

// ---------------------------------------------------------------- Lidl

/// Nächste Lidl-Filiale im Umkreis der PLZ; None, wenn keine existiert.
pub fn lidl_branch(plz: &str) -> Result<Option<Market>> {
    let (lat, lon) = geocode_plz(plz)?;
    let url = format!(
        "https://spatial.virtualearth.net/REST/v1/data/{LIDL_DATASET}/Filialdaten-SEC/Filialdaten-SEC\
         ?key={LIDL_KEY}&$filter=Adresstyp%20Eq%201&spatialFilter=nearby({lat},{lon},{CUTOFF_KM})\
         &$select=EntityID,ShownStoreName,Locality,Latitude,Longitude&$format=json&$top=1"
    );
    util::polite_pause(&url);
    let raw: serde_json::Value = util::blocking_client()?
        .get(&url)
        .send()
        .with_context(|| util::ctx("Lidl", "Filialsuche", &url))?
        .error_for_status()
        .with_context(|| util::ctx("Lidl", "Filialsuche (HTTP-Status)", &url))?
        .json()
        .with_context(|| util::ctx("Lidl", "Filialsuche JSON parsen", &url))?;
    parse_virtualearth(&raw)
}

/// Erste Filiale einer Bing-SDS-Antwort als Market; None bei leerer Liste,
/// Fehler bei unerwartetem Format (damit der Aufrufer auf den Platzhalter
/// zurückfällt statt die Kette fälschlich abzumelden).
pub fn parse_virtualearth(raw: &serde_json::Value) -> Result<Option<Market>> {
    let results = raw
        .pointer("/d/results")
        .and_then(|v| v.as_array())
        .context("Bing-SDS-Antwort ohne d.results")?;
    let Some(store) = results.first() else {
        return Ok(None);
    };
    let id = store.get("EntityID").and_then(|v| v.as_str()).context("EntityID fehlt")?;
    // ShownStoreName ist oft leer — dann die Stadt (Locality), damit nicht
    // "Lidl " als Filialname in der App landet.
    let name = store
        .get("ShownStoreName")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            store
                .get("Locality")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("Filiale");
    Ok(Some(
        Market::new(format!("LIDL_{id}"), format!("Lidl {name}")).with_geo(
            store.get("Latitude").and_then(|v| v.as_f64()),
            store.get("Longitude").and_then(|v| v.as_f64()),
        ),
    ))
}

/// Absatzregion (AR) der nächsten Lidl-Filiale einer PLZ — der numerische
/// Regionsschlüssel, unter dem der Wochenprospekt regionalisiert wird. None,
/// wenn keine Filiale im Umkreis liegt oder das Feld fehlt.
///
/// Das Feld `AR` im Bing-SDS-Datensatz (`Filialdaten-SEC`) entspricht 1:1 dem
/// `regions[].code` im Flyer-JSON von endpoints.leaflets.schwarz: jede AR liegt
/// in genau einer der ~14 Wochenvarianten. Damit wird aus der PLZ die korrekte
/// Prospektvariante bestimmt (siehe lidl_prospekt::current_slug).
pub fn lidl_region_code(plz: &str) -> Result<Option<String>> {
    let (lat, lon) = geocode_plz(plz)?;
    let url = format!(
        "https://spatial.virtualearth.net/REST/v1/data/{LIDL_DATASET}/Filialdaten-SEC/Filialdaten-SEC\
         ?key={LIDL_KEY}&$filter=Adresstyp%20Eq%201&spatialFilter=nearby({lat},{lon},{CUTOFF_KM})\
         &$select=EntityID,AR&$format=json&$top=1"
    );
    util::polite_pause(&url);
    let raw: serde_json::Value = util::blocking_client()?
        .get(&url)
        .send()
        .with_context(|| util::ctx("Lidl", "Regions-Lookup", &url))?
        .error_for_status()
        .with_context(|| util::ctx("Lidl", "Regions-Lookup (HTTP-Status)", &url))?
        .json()
        .with_context(|| util::ctx("Lidl", "Regions-Lookup JSON parsen", &url))?;
    Ok(parse_region_code(&raw))
}

/// AR-Feld der ersten Filiale aus einer Bing-SDS-Antwort als String
/// ("20"); None bei leerer Liste oder fehlendem Feld. AR kann als Zahl oder
/// String ausgeliefert werden — beides wird normalisiert.
pub fn parse_region_code(raw: &serde_json::Value) -> Option<String> {
    let store = raw.pointer("/d/results")?.as_array()?.first()?;
    match store.get("AR")? {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------- ALDI (Uberall)

pub fn aldi_nord_branch(plz: &str) -> Result<Option<Market>> {
    uberall_branch(plz, ALDI_NORD_KEY, "ALDI Nord", "ALDI_NORD")
}

pub fn aldi_sued_branch(plz: &str) -> Result<Option<Market>> {
    uberall_branch(plz, ALDI_SUED_KEY, "ALDI SÜD", "ALDI_SUED")
}

fn uberall_branch(plz: &str, key: &str, chain: &str, id_prefix: &str) -> Result<Option<Market>> {
    let (lat, lon) = geocode_plz(plz)?;
    let url = format!("https://uberall.com/api/storefinders/{key}/locations?lat={lat}&lng={lon}&max=1");
    util::polite_pause(&url);
    let raw: serde_json::Value = util::blocking_client()?
        .get(&url)
        .send()
        .with_context(|| util::ctx(chain, "Filialsuche", &url))?
        .error_for_status()
        .with_context(|| util::ctx(chain, "Filialsuche (HTTP-Status)", &url))?
        .json()
        .with_context(|| util::ctx(chain, "Filialsuche JSON parsen", &url))?;
    parse_uberall(&raw, chain, id_prefix)
}

/// Nächste Filiale einer Uberall-Antwort als Market, sofern innerhalb des
/// Cutoffs (`distance` in Metern); None ohne Treffer im Umkreis.
pub fn parse_uberall(
    raw: &serde_json::Value,
    chain: &str,
    id_prefix: &str,
) -> Result<Option<Market>> {
    if raw.get("status").and_then(|v| v.as_str()) != Some("SUCCESS") {
        bail!("Uberall-Antwort ohne status=SUCCESS: {}", raw);
    }
    let locations = raw
        .pointer("/response/locations")
        .and_then(|v| v.as_array())
        .context("Uberall-Antwort ohne response.locations")?;
    let Some(store) = locations.first() else {
        return Ok(None);
    };
    if let Some(dist) = store.get("distance").and_then(|v| v.as_f64()) {
        if dist > CUTOFF_KM * 1000.0 {
            return Ok(None);
        }
    }
    let id = store
        .get("identifier")
        .and_then(|v| v.as_str())
        .map(|i| format!("{id_prefix}_{i}"))
        .unwrap_or_else(|| format!("{id_prefix}_DE"));
    let name = match store.get("city").and_then(|v| v.as_str()) {
        Some(city) => format!("{chain} {city}"),
        None => chain.to_string(),
    };
    Ok(Some(Market::new(id, name).with_geo(
        store.get("lat").and_then(|v| v.as_f64()),
        store.get("lng").and_then(|v| v.as_f64()),
    )))
}

// ---------------------------------------------------------------- Fallback

/// Finder-Ergebnis auf das Sync-Verhalten abbilden: Treffer -> echte Filiale,
/// sauberes "keine Filiale" -> None (Kette wird für die Region nicht
/// registriert), Fehler -> WARN + nationaler Platzhalter.
pub fn resolve(
    chain: &str,
    found: Result<Option<Market>>,
    national: Market,
) -> Option<Market> {
    match found {
        Ok(Some(market)) => Some(market),
        Ok(None) => None,
        Err(e) => {
            eprintln!(
                "WARNUNG [{chain}] Filialsuche fehlgeschlagen ({e:#}) — nutze nationalen Platzhalter."
            );
            Some(national)
        }
    }
}
