-- Which library each scryer title belongs to, learned whenever the bridge sees
-- the title. scryer sends `title_added` and `title_deleted` without a path, so
-- these events are routed by title id instead of by root-folder prefix. `path`
-- is the title folder from the last routed event that carried one; it fills
-- `folderPath` / `path` on a delete.
CREATE TABLE titles (
    title_id TEXT PRIMARY KEY,
    library_id TEXT NOT NULL,
    path TEXT
);
