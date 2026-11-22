//! SQLite access (server only).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Row, params};
use unicode_normalization::UnicodeNormalization;

use std::collections::{BTreeMap, HashSet};

use crate::model::{DAY, Product, Removed, State, Suggested, is_due, usual_interval};

/// Full schema (drops and recreates the tables), used by `init_db`.
pub const CREATE_TABLES: &str = include_str!("../CreateTables.sql");

/// Non-destructive schema bootstrap, run by the server at startup.
pub const ENSURE_TABLES: &str = "
CREATE TABLE IF NOT EXISTS spesa (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	name TEXT UNIQUE NOT NULL,
	quantity INT NOT NULL,
	category INT NOT NULL,
	count INT NOT NULL,
	toTake BOOLEAN NOT NULL
);
CREATE TABLE IF NOT EXISTS history (
	id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT UNIQUE,
	spesa_id INTEGER NOT NULL,
	taken_at INTEGER
);
CREATE TABLE IF NOT EXISTS postponed (
	spesa_id INTEGER PRIMARY KEY,
	until INTEGER NOT NULL
);
";

/// Fills `history.taken_at` on insert (see CreateTables.sql).
const TAKEN_AT_TRIGGER: &str = "
CREATE TRIGGER IF NOT EXISTS history_taken_at AFTER INSERT ON history
WHEN NEW.taken_at IS NULL
BEGIN
	UPDATE history SET taken_at = CAST(strftime('%s', 'now') AS INTEGER) WHERE id = NEW.id;
END;
";

/// How many recently taken products are shown.
const RECENTLY_REMOVED: i64 = 20;

/// Days of history kept: enough to learn monthly (and rarer) habits.
const HISTORY_DAYS: i64 = 400;

static DB_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Path of `database.db` inside `dir`.
pub fn db_path(dir: &Path) -> PathBuf {
    dir.join("database.db")
}

/// Opens the database with a 5 s busy timeout (same as Python's `sqlite3`).
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn)
}

/// Selects `<dir>/database.db` as the server database, creating missing tables.
pub fn init(dir: &Path) -> anyhow::Result<()> {
    let path = db_path(dir);
    ensure_schema(&open(&path)?)?;
    DB_PATH
        .set(path)
        .map_err(|_| anyhow::anyhow!("database already initialized"))
}

/// Creates missing tables and upgrades databases created by the Python app
/// (adds `history.taken_at` and its trigger). Never deletes data.
pub fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(ENSURE_TABLES)?;
    let has_taken_at = conn
        .prepare("SELECT 1 FROM pragma_table_info('history') WHERE name = 'taken_at'")?
        .exists([])?;
    if !has_taken_at {
        conn.execute_batch("ALTER TABLE history ADD COLUMN taken_at INTEGER")?;
    }
    conn.execute_batch(TAKEN_AT_TRIGGER)
}

/// Runs database work on a fresh connection (blocking, for background threads).
pub fn with_db<T>(f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> anyhow::Result<T> {
    let path = DB_PATH.get().ok_or_else(|| anyhow::anyhow!("database not initialized"))?;
    Ok(f(&mut open(path)?)?)
}

/// Runs blocking database work on a fresh connection, off the async runtime.
pub async fn run<T, F>(f: F) -> Result<T, leptos::prelude::ServerFnError>
where
    T: Send + 'static,
    F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
{
    use leptos::prelude::ServerFnError;
    let path = DB_PATH
        .get()
        .ok_or_else(|| ServerFnError::new("database not initialized"))?
        .clone();
    tokio::task::spawn_blocking(move || f(&mut open(&path)?))
        .await
        .map_err(ServerFnError::new)?
        .map_err(ServerFnError::new)
}

// ---------------------------------------------------------------------------
// Change detection

/// Watches the database for committed changes, made by anyone: this server,
/// another program (e.g. the Python app), the `sqlite3` shell, or the file being
/// replaced. The returned counter is incremented at every change.
///
/// A dedicated connection polls `PRAGMA data_version`, which changes whenever
/// another connection commits.
pub fn watch_changes(interval: Duration) -> tokio::sync::watch::Receiver<u64> {
    let path = DB_PATH.get().expect("db::init must be called first").clone();
    let (tx, rx) = tokio::sync::watch::channel(0u64);
    std::thread::Builder::new()
        .name("db-watcher".into())
        .spawn(move || {
            let mut watcher = Watcher::default();
            while !tx.is_closed() {
                std::thread::sleep(interval);
                match watcher.poll(&path) {
                    Ok(true) => tx.send_modify(|n| *n += 1),
                    Ok(false) => {}
                    Err(e) => watcher.fail(&e),
                }
            }
        })
        .expect("cannot start the db-watcher thread");
    rx
}

#[derive(Default)]
struct Watcher {
    /// Open connection, last `data_version` and identity of the watched file.
    current: Option<(Connection, i64, u64)>,
    /// Whether a first successful poll happened (the initial open is not a change).
    started: bool,
    /// Last error, logged only once.
    last_error: Option<String>,
}

impl Watcher {
    /// Returns `true` if the database changed since the previous call.
    fn poll(&mut self, path: &Path) -> anyhow::Result<bool> {
        let id = file_id(path)?;
        if let Some((conn, version, watched_id)) = &mut self.current
            && *watched_id == id
        {
            let v: i64 = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
            return Ok(std::mem::replace(version, v) != v);
        }
        // First poll, file replaced, or recovering from an error: (re)open.
        let conn = open(path)?;
        let v: i64 = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        self.current = Some((conn, v, id));
        self.last_error = None;
        Ok(std::mem::replace(&mut self.started, true))
    }

    fn fail(&mut self, e: &anyhow::Error) {
        self.current = None;
        let msg = format!("{e:#}");
        if self.last_error.as_ref() != Some(&msg) {
            eprintln!("db-watcher: {msg}");
            self.last_error = Some(msg);
        }
    }
}

/// Identity of the file, to notice when it is replaced (inode on Unix).
fn file_id(path: &Path) -> std::io::Result<u64> {
    let meta = std::fs::metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(meta.ino())
    }
    #[cfg(not(unix))]
    {
        Ok(meta.len())
    }
}

// ---------------------------------------------------------------------------
// Queries

/// Integer value of a column that may contain text (SQLite is dynamically typed).
fn int_of(v: &SqlValue) -> i64 {
    match v {
        SqlValue::Integer(i) => *i,
        SqlValue::Real(f) => f.trunc() as i64,
        SqlValue::Text(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

fn product_from_row(r: &Row) -> rusqlite::Result<Product> {
    let name = match r.get::<_, SqlValue>(1)? {
        SqlValue::Text(s) => s,
        other => int_of(&other).to_string(),
    };
    Ok(Product {
        id: r.get(0)?,
        name,
        category: int_of(&r.get(2)?),
        count: int_of(&r.get(3)?),
    })
}

fn products(conn: &Connection, sql: &str) -> rusqlite::Result<Vec<Product>> {
    let mut stmt = conn.prepare(sql)?;
    stmt.query_map([], product_from_row)?.collect()
}

/// Products taken most recently (and not back on the list), newest first,
/// with a human-readable local time ("oggi 10:32", "ieri 18:05", "18/11 09:15").
fn recently_removed(conn: &Connection) -> rusqlite::Result<Vec<Removed>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.name, s.category, s.count,
            CASE
              WHEN h.taken_at IS NULL THEN ''
              WHEN date(h.taken_at, 'unixepoch', 'localtime') = date('now', 'localtime')
                THEN 'oggi ' || strftime('%H:%M', h.taken_at, 'unixepoch', 'localtime')
              WHEN date(h.taken_at, 'unixepoch', 'localtime') = date('now', 'localtime', '-1 day')
                THEN 'ieri ' || strftime('%H:%M', h.taken_at, 'unixepoch', 'localtime')
              ELSE strftime('%d/%m %H:%M', h.taken_at, 'unixepoch', 'localtime')
            END
         FROM (SELECT spesa_id, MAX(id) AS last FROM history GROUP BY spesa_id) AS latest
         JOIN history AS h ON h.id = latest.last
         JOIN spesa AS s ON s.id = latest.spesa_id
         WHERE s.toTake = 0
         ORDER BY h.id DESC
         LIMIT ?",
    )?;
    stmt.query_map([RECENTLY_REMOVED], |r| {
        Ok(Removed { product: product_from_row(r)?, when: r.get(4)? })
    })?
    .collect()
}

/// Current time, Unix seconds.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Catalog products due to be bought again at time `now`, most overdue first
/// (products dismissed with "non ora" are left out until their time).
fn suggested(conn: &Connection, catalog: &[Product], now: i64) -> rusqlite::Result<Vec<Suggested>> {
    let mut purchases: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    let mut stmt = conn.prepare(
        "SELECT spesa_id, taken_at FROM history WHERE taken_at IS NOT NULL ORDER BY taken_at",
    )?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
        let (id, at) = row?;
        purchases.entry(id).or_default().push(at);
    }
    let postponed: HashSet<i64> = conn
        .prepare("SELECT spesa_id FROM postponed WHERE until > ?")?
        .query_map([now], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    let mut due: Vec<(f64, Suggested)> = catalog
        .iter()
        .filter(|p| !postponed.contains(&p.id))
        .filter_map(|p| {
            let times = purchases.get(&p.id)?;
            let interval = usual_interval(times)?;
            let last = *times.last()?;
            is_due(last, interval, now).then(|| {
                let overdue = (now - last) as f64 / interval as f64;
                let every_days = ((interval + DAY / 2) / DAY).max(1);
                (overdue, Suggested { product: p.clone(), every_days, days_ago: (now - last) / DAY })
            })
        })
        .collect();
    due.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(due.into_iter().map(|(_, s)| s).collect())
}

/// Catalog, shopping list, recently taken and suggested products.
pub fn load_state(conn: &Connection) -> rusqlite::Result<State> {
    load_state_at(conn, now())
}

fn load_state_at(conn: &Connection, now: i64) -> rusqlite::Result<State> {
    let catalog = products(
        conn,
        "SELECT id, name, category, count FROM spesa WHERE toTake=0 ORDER BY count DESC, name",
    )?;
    Ok(State {
        removed: recently_removed(conn)?,
        suggested: suggested(conn, &catalog, now)?,
        catalog,
        list: products(
            conn,
            "SELECT id, name, category, count FROM spesa WHERE toTake=1 ORDER BY category, name",
        )?,
    })
}

/// "Non ora": hides a suggestion for `days` days.
pub fn postpone(conn: &Connection, id: i64, days: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO postponed (spesa_id, until) VALUES (?, ?)",
        params![id, now() + days.max(1) * DAY],
    )?;
    Ok(())
}

/// Number of products on the shopping list.
pub fn count_on_list(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM spesa WHERE toTake=1", [], |r| r.get(0))
}

/// Puts a product on the list by name, creating it if it does not exist.
pub fn add_product(conn: &Connection, name: &str, category: i64) -> rusqlite::Result<()> {
    let name = normalize_name(name);
    if name.is_empty() {
        return Ok(());
    }
    let existing: Option<i64> = conn
        .query_row("SELECT id FROM spesa WHERE name=?", [&name], |r| r.get(0))
        .optional()?;
    match existing {
        Some(id) => conn.execute(
            "UPDATE spesa SET toTake=1, category=?, count=count+1 WHERE id=?",
            params![category, id],
        )?,
        None => conn.execute(
            "INSERT INTO spesa (name, quantity, category, count, toTake) VALUES (?, 0, ?, 0, 1)",
            params![name, category],
        )?,
    };
    Ok(())
}

/// Puts a product on the list from a voice assistant, which gives only a name
/// (often lowercase): an existing product is matched ignoring case and keeps
/// its name and category; a new one gets category 0 ("Varie").
pub fn add_from_voice(conn: &mut Connection, name: &str) -> rusqlite::Result<()> {
    let name = capitalize(&normalize_name(name));
    if name.is_empty() {
        return Ok(());
    }
    let existing: Option<i64> = conn
        .query_row("SELECT id FROM spesa WHERE name = ? COLLATE NOCASE", [&name], |r| r.get(0))
        .optional()?;
    match existing {
        // counts as a purchase only if it was not already on the list
        Some(id) => conn.execute(
            "UPDATE spesa SET count = count + (toTake = 0), toTake = 1 WHERE id = ?",
            [id],
        )?,
        None => conn.execute(
            "INSERT INTO spesa (name, quantity, category, count, toTake) VALUES (?, 0, 0, 0, 1)",
            [&name],
        )?,
    };
    Ok(())
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Puts a catalog product back on the list, possibly renaming it.
/// An empty name deletes the product.
pub fn put_back(conn: &Connection, id: i64, name: &str, category: i64) -> rusqlite::Result<()> {
    let name = normalize_name(name);
    if name.is_empty() {
        conn.execute("DELETE FROM spesa WHERE id=?", [id])?;
    } else {
        conn.execute(
            "UPDATE spesa SET toTake=1, name=?, category=?, count=count+1 WHERE id=?",
            params![name, category, id],
        )?;
    }
    Ok(())
}

/// Undoes "taken": the product goes back on the list and its last history
/// entry is removed. Does nothing if the product is already on the list.
pub fn restore(conn: &mut Connection, id: i64) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    if tx.execute("UPDATE spesa SET toTake=1 WHERE id=? AND toTake=0", [id])? > 0 {
        tx.execute(
            "DELETE FROM history WHERE id = (SELECT MAX(id) FROM history WHERE spesa_id=?)",
            [id],
        )?;
    }
    tx.commit()
}

/// Changes the category of a product.
pub fn set_category(conn: &Connection, id: i64, category: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE spesa SET category=? WHERE id=?", params![category, id])?;
    Ok(())
}

/// Marks a product as bought, recording it in the history (kept for
/// `HISTORY_DAYS` days) and clearing a "non ora" on it.
pub fn take(conn: &mut Connection, id: i64) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("UPDATE spesa SET toTake=0 WHERE id=?", [id])?;
    tx.execute("INSERT INTO history (spesa_id) VALUES (?)", [id])?;
    tx.execute("DELETE FROM postponed WHERE spesa_id=?", [id])?;
    tx.execute(
        "DELETE FROM history WHERE taken_at < CAST(strftime('%s', 'now') AS INTEGER) - ?",
        [HISTORY_DAYS * DAY],
    )?;
    tx.commit()
}

// ---------------------------------------------------------------------------
// Text helpers

/// NFKC normalization, typographic apostrophes turned into `'`, surrounding spaces removed.
pub fn normalize_name(s: &str) -> String {
    s.nfkc()
        .collect::<String>()
        .replace(['\u{2019}', '\u{2018}'], "'")
        .trim()
        .to_string()
}

/// Parses one line of `lista.txt` (`[x] <quantity> <category> <name...>`).
///
/// Returns `(quantity, category, name)`, or `None` if the line is malformed.
pub fn parse_lista_line(line: &str) -> Option<(i64, i64, String)> {
    let cleaned = line.replace("[x]", "").replace("[]", "").replace("[ ]", "");
    let mut parts = cleaned.split_whitespace();
    let quantity = parts.next()?.parse().ok()?;
    let category = parts.next()?.parse().ok()?;
    let name = parts.collect::<Vec<_>>().join(" ");
    (!name.is_empty()).then_some((quantity, category, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(CREATE_TABLES).unwrap();
        conn
    }

    #[test]
    fn parses_lista_lines() {
        assert_eq!(
            parse_lista_line("[ ] 1 66 Mozzarella 2 conf \n"),
            Some((1, 66, "Mozzarella 2 conf".into()))
        );
        assert_eq!(parse_lista_line("[x] 750 71 Tritata"), Some((750, 71, "Tritata".into())));
        assert_eq!(parse_lista_line("[x] 1 66"), None);
        assert_eq!(parse_lista_line("[x] a 66 Pane"), None);
        assert_eq!(parse_lista_line(""), None);
    }

    #[test]
    fn normalizes_names() {
        assert_eq!(normalize_name(" l\u{2019}olio "), "l'olio");
        assert_eq!(normalize_name("\u{FB01}letto"), "filetto");
    }

    #[test]
    fn shopping_flow() {
        let mut conn = memory_db();
        add_product(&conn, "Pane", 35).unwrap();
        add_product(&conn, "Latte", 50).unwrap();
        add_product(&conn, "  ", 50).unwrap(); // ignored
        let state = load_state(&conn).unwrap();
        assert_eq!(state.list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Pane", "Latte"]);
        assert!(state.catalog.is_empty());

        let pane = state.list[0].id;
        take(&mut conn, pane).unwrap();
        assert_eq!(count_on_list(&conn).unwrap(), 1);

        // adding an existing product by name increments its counter
        add_product(&conn, "Pane", 36).unwrap();
        let state = load_state(&conn).unwrap();
        assert_eq!(state.list[0], Product { id: pane, name: "Pane".into(), category: 36, count: 1 });

        set_category(&conn, pane, 70).unwrap();
        take(&mut conn, pane).unwrap();
        put_back(&conn, pane, "Pane integrale", 35).unwrap();
        let state = load_state(&conn).unwrap();
        assert_eq!(state.list[0].name, "Pane integrale");
        assert_eq!(state.list[0].count, 2);

        // empty name deletes
        put_back(&conn, pane, "", 35).unwrap();
        assert_eq!(load_state(&conn).unwrap().list.len(), 1);
    }

    #[test]
    fn watcher_sees_other_connections_and_replaced_files() {
        let dir = std::env::temp_dir().join(format!("spesa-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("database.db");
        let _ = std::fs::remove_file(&path);
        open(&path).unwrap().execute_batch(CREATE_TABLES).unwrap();

        let mut w = Watcher::default();
        assert!(!w.poll(&path).unwrap(), "initial open is not a change");
        assert!(!w.poll(&path).unwrap());

        add_product(&open(&path).unwrap(), "Pane", 35).unwrap();
        assert!(w.poll(&path).unwrap(), "commit by another connection");
        assert!(!w.poll(&path).unwrap());

        // file replaced (e.g. copied over with scp)
        let tmp = dir.join("new.db");
        open(&tmp).unwrap().execute_batch(CREATE_TABLES).unwrap();
        std::fs::rename(&tmp, &path).unwrap();
        assert!(w.poll(&path).unwrap(), "file replaced");

        // file missing, then back
        std::fs::rename(&path, &tmp).unwrap();
        w.poll(&path).unwrap_err();
        w.fail(&anyhow::anyhow!("missing"));
        std::fs::rename(&tmp, &path).unwrap();
        assert!(w.poll(&path).unwrap(), "file back after an error");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recently_removed_and_restore() {
        let mut conn = memory_db();
        for name in ["Pane", "Latte", "Uova"] {
            add_product(&conn, name, 35).unwrap();
        }
        take(&mut conn, 1).unwrap();
        take(&mut conn, 3).unwrap();
        let removed = load_state(&conn).unwrap().removed;
        let names: Vec<_> = removed.iter().map(|r| r.product.name.as_str()).collect();
        assert_eq!(names, ["Uova", "Pane"], "newest first");
        assert!(removed[0].when.starts_with("oggi "), "{}", removed[0].when);

        restore(&mut conn, 3).unwrap();
        let state = load_state(&conn).unwrap();
        assert_eq!(state.removed.len(), 1);
        assert!(state.list.iter().any(|p| p.name == "Uova"));
        let history: i64 = conn.query_row("SELECT COUNT(*) FROM history", [], |r| r.get(0)).unwrap();
        assert_eq!(history, 1, "restore removes the history entry");
        restore(&mut conn, 3).unwrap(); // already on the list: no-op
        assert_eq!(load_state(&conn).unwrap().list.len(), 2);
    }

    #[test]
    fn adds_from_voice() {
        let mut conn = memory_db();
        add_product(&conn, "Latte soia", 52).unwrap();
        take(&mut conn, 1).unwrap();

        add_from_voice(&mut conn, "latte soia").unwrap(); // existing, any case
        add_from_voice(&mut conn, "  uova ").unwrap(); // new
        add_from_voice(&mut conn, "UOVA").unwrap(); // already on the list
        add_from_voice(&mut conn, " ").unwrap(); // ignored

        let state = load_state(&conn).unwrap();
        let list: Vec<_> = state.list.iter().map(|p| (p.name.as_str(), p.category, p.count)).collect();
        assert_eq!(list, [("Uova", 0, 0), ("Latte soia", 52, 1)]);
    }

    #[test]
    fn upgrades_python_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE history (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT UNIQUE, spesa_id INTEGER NOT NULL);
             INSERT INTO history (spesa_id) VALUES (7);",
        )
        .unwrap();
        ensure_schema(&conn).unwrap();
        ensure_schema(&conn).unwrap(); // idempotent
        // what the Python app does: insert without a time
        conn.execute("INSERT INTO history (spesa_id) VALUES (8)", []).unwrap();
        let times: Vec<Option<i64>> = conn
            .prepare("SELECT taken_at FROM history ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(times[0], None, "old rows have no time");
        assert!(times[1].unwrap() > 1_700_000_000, "trigger sets the time");
    }

    #[test]
    fn history_keeps_the_last_400_days() {
        let mut conn = memory_db();
        add_product(&conn, "Pane", 35).unwrap();
        let t = now();
        for days_ago in [500, 401, 399, 10] {
            conn.execute(
                "INSERT INTO history (spesa_id, taken_at) VALUES (1, ?)",
                [t - days_ago * DAY],
            )
            .unwrap();
        }
        take(&mut conn, 1).unwrap();
        let kept: i64 = conn.query_row("SELECT COUNT(*) FROM history", [], |r| r.get(0)).unwrap();
        assert_eq!(kept, 3, "399 and 10 days ago, plus today");
    }

    #[test]
    fn suggests_by_frequency() {
        let mut conn = memory_db();
        for name in ["Latte", "Detersivo", "Pane", "Nuovo"] {
            add_product(&conn, name, 35).unwrap();
        }
        conn.execute("UPDATE spesa SET toTake=0", []).unwrap();
        let now = 1_000 * DAY;
        let buy = |id: i64, days_ago: i64| {
            conn.execute("INSERT INTO history (spesa_id, taken_at) VALUES (?, ?)", [id, now - days_ago * DAY]).unwrap();
        };
        // Latte weekly, last 8 days ago -> due (overdue 8/7)
        for d in [36, 29, 22, 15, 8] { buy(1, d) }
        // Detersivo monthly, last 28 days ago -> due (28/30 >= 0.9)
        for d in [88, 58, 28] { buy(2, d) }
        // Pane weekly, last 3 days ago -> not due
        for d in [17, 10, 3] { buy(3, d) }
        // Nuovo bought once -> unknown frequency
        buy(4, 40);

        let s = load_state_at(&conn, now).unwrap().suggested;
        let got: Vec<_> = s.iter().map(|s| (s.product.name.as_str(), s.every_days, s.days_ago)).collect();
        assert_eq!(got, [("Latte", 7, 8), ("Detersivo", 30, 28)]);

        // products on the list are not suggested
        conn.execute("UPDATE spesa SET toTake=1 WHERE id=1", []).unwrap();
        assert_eq!(load_state_at(&conn, now).unwrap().suggested.len(), 1);
        conn.execute("UPDATE spesa SET toTake=0 WHERE id=1", []).unwrap();

        // "non ora" hides it until the time given
        conn.execute("INSERT INTO postponed (spesa_id, until) VALUES (1, ?)", [now + 7 * DAY]).unwrap();
        assert_eq!(load_state_at(&conn, now).unwrap().suggested.len(), 1);
        let later: Vec<_> = load_state_at(&conn, now + 8 * DAY).unwrap().suggested
            .into_iter().map(|s| s.product.name).collect();
        assert_eq!(later, ["Latte", "Pane", "Detersivo"], "after the postponement (Pane is due by then)");
        // taking the product clears the postponement
        take(&mut conn, 1).unwrap();
        let left: i64 = conn.query_row("SELECT COUNT(*) FROM postponed", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
    }
}
