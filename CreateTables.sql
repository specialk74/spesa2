DROP TABLE IF EXISTS spesa;

CREATE TABLE spesa (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	name TEXT UNIQUE NOT NULL,
	quantity INT NOT NULL,
	category INT NOT NULL,
	count INT NOT NULL,
	toTake BOOLEAN NOT NULL
);

DROP TABLE IF EXISTS history;

CREATE TABLE history (
	id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT UNIQUE,
	spesa_id INTEGER NOT NULL,
	taken_at INTEGER
);

-- Suggestions dismissed with "non ora", until the given time (Unix seconds).
DROP TABLE IF EXISTS postponed;
CREATE TABLE postponed (
	spesa_id INTEGER PRIMARY KEY,
	until INTEGER NOT NULL
);

-- Time (Unix seconds) at which the product was taken, also when the row
-- is inserted by programs that do not set it (e.g. the old Python app).
CREATE TRIGGER history_taken_at AFTER INSERT ON history
WHEN NEW.taken_at IS NULL
BEGIN
	UPDATE history SET taken_at = CAST(strftime('%s', 'now') AS INTEGER) WHERE id = NEW.id;
END;
