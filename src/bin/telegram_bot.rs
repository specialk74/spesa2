//! Sends "Spesa finita..." to a Telegram chat when the shopping list becomes empty
//! (Rust port of `telegram_bot.py`, meant to be run periodically, e.g. from cron).
//!
//! Reads `<path>/database.db`, `<path>/last_number.txt` and `<path>/config.ini`:
//!
//! ```ini
//! [DEFAULT]
//! TELEGRAM_TOKEN = 123456:ABC...
//! TELEGRAM_CHAT_ID = -100123456
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Parser;
use spesa::db::{count_on_list, db_path, open};

#[derive(Parser, Debug)]
#[command(about = "Notify a Telegram chat when the shopping list is done")]
struct Args {
    /// Directory containing database.db, last_number.txt and config.ini
    #[arg(long)]
    path: PathBuf,
}

const MESSAGE: &str = "Spesa finita...";

/// Minimal INI reader: returns the keys (upper-cased) of the `[DEFAULT]` section.
fn read_default_section(path: &Path) -> anyhow::Result<HashMap<String, String>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let mut in_default = false;
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_default = section.trim() == "DEFAULT";
            continue;
        }
        if !in_default {
            continue;
        }
        if let Some(pos) = line.find(['=', ':']) {
            let (k, v) = line.split_at(pos);
            out.insert(k.trim().to_uppercase(), v[1..].trim().to_string());
        }
    }
    Ok(out)
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let last_number_file = args.path.join("last_number.txt");

    let num_take = count_on_list(&open(&db_path(&args.path))?)?;

    let last_number = std::fs::read_to_string(&last_number_file)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok());
    if last_number == Some(num_take) {
        return Ok(());
    }

    if num_take == 0 {
        let config = read_default_section(&args.path.join("config.ini"))?;
        let Some(token) = config.get("TELEGRAM_TOKEN") else {
            println!("TELEGRAM_TOKEN None");
            return Ok(());
        };
        let Some(chat_id) = config.get("TELEGRAM_CHAT_ID") else {
            println!("TELEGRAM_CHAT_ID None");
            return Ok(());
        };

        let url = format!("https://api.telegram.org/bot{token}/sendMessage");
        let response = ureq::get(&url)
            .query("chat_id", chat_id)
            .query("text", MESSAGE)
            .call()
            .context("sending Telegram message")?
            .body_mut()
            .read_to_string()?;
        println!("{response}");
    }

    std::fs::write(&last_number_file, num_take.to_string())
        .with_context(|| format!("writing {}", last_number_file.display()))?;
    Ok(())
}
