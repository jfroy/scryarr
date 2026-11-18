CREATE TABLE arr_ids (
    arr_id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    library_id TEXT NOT NULL,
    scryer_key TEXT NOT NULL,
    UNIQUE (kind, library_id, scryer_key)
);

CREATE TABLE notifications (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    library_id TEXT NOT NULL,
    flavor TEXT NOT NULL,
    body TEXT NOT NULL
);

CREATE TABLE library_roots (
    library_id TEXT NOT NULL,
    facet TEXT NOT NULL,
    path TEXT NOT NULL,
    PRIMARY KEY (library_id, path)
);
