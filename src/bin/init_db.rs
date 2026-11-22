//! Creates `database.db` and fills it from `lista.txt`.
//!
//! WARNING: the existing `spesa` and `history` tables are dropped.

use std::path::PathBuf;

use clap::Parser;
use spesa::db::{CREATE_TABLES, open, parse_lista_line};

#[derive(Parser, Debug)]
#[command(about = "Initialize the SpesaApp database from a product list")]
struct Args {
    /// Database file to (re)create
    #[arg(long, default_value = "database.db")]
    db: PathBuf,
    /// Product list, one `[x] <quantity> <category> <name>` per line
    #[arg(long, default_value = "lista.txt")]
    lista: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut conn = open(&args.db)?;
    conn.execute_batch(CREATE_TABLES)?;

    let content = std::fs::read_to_string(&args.lista)?;
    let tx = conn.transaction()?;
    let mut inserted = 0usize;
    for (quantity, category, name) in content.lines().filter_map(parse_lista_line) {
        // Duplicate names violate the UNIQUE constraint and are skipped.
        let ok = tx
            .execute(
                "INSERT INTO spesa (name, quantity, category, count, toTake) VALUES (?, ?, ?, 0, 0)",
                (&name, quantity, category),
            )
            .is_ok();
        if ok {
            println!("{quantity:>4} {category:>3}  {name}");
            inserted += 1;
        }
    }
    tx.commit()?;
    eprintln!("{inserted} products inserted into {}", args.db.display());
    Ok(())
}
