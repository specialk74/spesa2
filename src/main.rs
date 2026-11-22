//! SpesaApp web server.
//!
//! Usage: `spesa --port 9080 --path <data-dir> [--site <dir>] [--alexa <login.json>]`

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::response::Redirect;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use tokio::sync::watch;
use tokio_stream::{Stream, StreamExt, wrappers::WatchStream};
use clap::Parser;
use leptos::prelude::*;
use leptos_axum::{LeptosRoutes, generate_route_list};
use spesa::app::{App, shell};

#[derive(Parser, Debug)]
#[command(about = "SpesaApp - shopping list web server")]
struct Args {
    /// TCP port to listen on, on all interfaces (default: from Cargo.toml, 9080)
    #[arg(long)]
    port: Option<u16>,
    /// Directory containing database.db
    #[arg(long, default_value = ".")]
    path: PathBuf,
    /// Login file written by tools/alexa_login.py: enables the import of the
    /// Alexa shopping list (items are moved to SpesaApp and deleted from Alexa)
    #[arg(long)]
    alexa: Option<PathBuf>,
    /// Only show the items of the Alexa shopping list (read-only) and exit
    #[arg(long, requires = "alexa")]
    alexa_check: bool,
    /// Seconds between two checks of the Alexa shopping list
    #[arg(long, default_value_t = 30)]
    alexa_interval: u64,
    /// Directory with the web assets built by cargo-leptos (pkg/).
    /// Default: `site` next to the executable if present, otherwise target/site.
    #[arg(long)]
    site: Option<PathBuf>,
}

/// How often the database is checked for changes.
const WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// Server-Sent Events stream: one `db` event every time the database changes.
fn db_events(changes: watch::Receiver<u64>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = WatchStream::from_changes(changes)
        .map(|n| Ok(Event::default().event("db").data(n.to_string())));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// `site` directory next to the executable, used when deployed.
fn site_next_to_exe() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.join("site");
    dir.join("pkg").is_dir().then_some(dir)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if args.alexa_check {
        let login = args.alexa.as_deref().expect("required by clap");
        let items = spesa::alexa::Alexa::load(login)?
            .peek()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        println!("Lista della spesa di Alexa: {} elementi", items.len());
        for item in items {
            println!("  - {}", item.name);
        }
        return Ok(());
    }

    let mut options = get_configuration(None)?.leptos_options;
    if let Some(port) = args.port {
        options.site_addr = SocketAddr::from(([0, 0, 0, 0], port));
    }
    if let Some(site) = args.site.or_else(site_next_to_exe) {
        options.site_root = site.to_string_lossy().into_owned().into();
    }

    spesa::db::init(&args.path)?;
    let changes = spesa::db::watch_changes(WATCH_INTERVAL);
    if let Some(login) = &args.alexa {
        spesa::alexa::spawn(login.clone(), Duration::from_secs(args.alexa_interval.max(5)))?;
        println!("Alexa:    {} (every {} s)", login.display(), args.alexa_interval.max(5));
    }
    println!("Database: {}", spesa::db::db_path(&args.path).display());
    println!("Assets:   {}", options.site_root);

    let addr = options.site_addr;
    let routes = generate_route_list(App);
    let app = Router::new()
        .route("/", get(|| async { Redirect::permanent("/spesa") }))
        .route(spesa::app::EVENTS_URL, get(move || {
            let changes = changes.clone();
            async move { db_events(changes) }
        }))
        .leptos_routes(&options, routes, {
            let options = options.clone();
            move || shell(options.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(options);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Listening on http://{addr}/spesa");
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}
