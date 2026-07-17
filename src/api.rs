use std::sync::Arc;

use anyhow::Result;
use axum::Router;

// Hinweis zur Nebenläufigkeit: rusqlite::Connection ist nicht Sync und kann
// daher nicht als geteilter axum-State verwendet werden. Stattdessen hält der
// State nur den DB-Pfad und jeder Handler öffnet pro Request eine eigene
// Verbindung (db::open) — bei SQLite im WAL-Modus günstig und für parallele
// Lesezugriffe unproblematisch.
pub struct AppState {
    pub db_path: String,
}

pub fn router(db_path: String) -> Router {
    Router::new().with_state(Arc::new(AppState { db_path }))
}

/// HTTP-Server starten (blockiert bis zum Abbruch).
pub fn serve(port: u16, db_path: String) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
        println!("HTTP-API läuft auf http://0.0.0.0:{port} (DB: {db_path})");
        axum::serve(listener, router(db_path)).await?;
        Ok(())
    })
}
