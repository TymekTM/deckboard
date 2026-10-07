//! SQLite access for the Pulpit database (`~/pulpitApp/database.db`).
//!
//! M0 opens the file read-only: the original desktop app stays the writer
//! until the Rust core takes over persistence, so the two never fight over
//! the file (see docs/decisions.md, "single writer").

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DbError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database file not found: {0}")]
    NotFound(PathBuf),
    #[error("serialization error: {0}")]
    Serialize(String),
    #[error("malformed data: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, DbError>;

/// Row of the `Boards` table. Serde shape matches the `.boardjson` export of
/// the original app: the `type` column is `type`, every field optional on
/// import so files written by the original (any era) parse.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BoardRow {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub background: String,
    #[serde(default)]
    pub layout: i64,
    #[serde(default)]
    pub image: String,
    #[serde(default, deserialize_with = "de_lenient_int")]
    pub sort: i64,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub args: Option<String>,
    #[serde(default)]
    pub order: i64,
    #[serde(default)]
    pub width: i64,
    #[serde(default)]
    pub height: i64,
    #[serde(default)]
    pub converted: i64,
}

/// Largest board grid any surface will build. The import path rejects
/// wider/taller boards outright; the same bound belongs in the editor
/// UI and both wire builders (audit item C4). One definition here, so
/// the import bound and the wire clamps can never drift apart
/// (`pulpit_backend` re-exports it).
pub const MAX_BOARD_DIM: i64 = 32;

/// Pull a tile's placement back inside a W*H board grid (DESK-03):
/// size first (at least one cell, at most the whole board), then the
/// origin (>= 0, leaving room for the size). The order matters - an
/// origin-first clamp can still leave the tile overhanging when the
/// board shrank below the tile's size. Both dimensions are clamped to
/// [`MAX_BOARD_DIM`] first, so a junk board row cannot stretch a tile
/// back out.
pub fn clamp_placement(
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    width: i64,
    height: i64,
) -> (i64, i64, i64, i64) {
    let width = width.clamp(1, MAX_BOARD_DIM);
    let height = height.clamp(1, MAX_BOARD_DIM);
    let w = w.clamp(1, width);
    let h = h.clamp(1, height);
    (x.clamp(0, width - w), y.clamp(0, height - h), w, h)
}

/// Row of the `Shortcuts` table (one macro button/slider/wheel). Serde shape
/// matches the `macros` entries of a `.boardjson` export.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ButtonRow {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub board_id: i64,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub title_position: i64,
    #[serde(default)]
    pub title_color: Option<String>,
    #[serde(default)]
    pub title_box_color: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub icon_color: Option<String>,
    #[serde(default)]
    pub icon_color2: Option<String>,
    #[serde(default)]
    pub border_color: Option<String>,
    #[serde(default)]
    pub shape: i64,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub img: Option<String>,
    #[serde(default)]
    pub img2: Option<String>,
    #[serde(default)]
    pub icon2: Option<String>,
    #[serde(default)]
    pub color2: Option<String>,
    #[serde(default)]
    pub shape2: i64,
    #[serde(default)]
    pub border_color2: Option<String>,
    #[serde(default)]
    pub title_position2: i64,
    #[serde(default)]
    pub title_box_color2: Option<String>,
    #[serde(default)]
    pub title_color2: Option<String>,
    #[serde(default)]
    pub position: Option<i64>,
    #[serde(default)]
    pub position2: i64,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub x: Option<i64>,
    #[serde(default)]
    pub y: Option<i64>,
    #[serde(default)]
    pub w: i64,
    #[serde(default)]
    pub h: i64,
    #[serde(default)]
    pub options: Option<String>,
}

#[derive(Debug)]
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open the database read-only. Pass `None` to use `~/pulpitApp/database.db`.
    pub fn open_read_only(path: Option<&Path>) -> Result<Db> {
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => default_db_path(),
        };
        if !path.exists() {
            return Err(DbError::NotFound(path));
        }
        let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        tracing::info!(path = %path.display(), "opened pulpitApp database (read-only)");
        Ok(Db { conn })
    }

    /// Open for tests / tooling: read-write, creating an empty schema if needed.
    pub fn open_or_create(path: &Path) -> Result<Db> {
        let conn = Connection::open(path)?;
        create_schema(&conn)?;
        Ok(Db { conn })
    }

    pub fn get_boards(&self) -> Result<Vec<BoardRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, background, layout, image, sort, type, args, \
             COALESCE(\"order\", 0), width, height, converted \
             FROM Boards ORDER BY COALESCE(\"order\", 0), id",
        )?;
        let rows = stmt
            .query_map([], map_board_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_board(&self, id: i64) -> Result<Option<BoardRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, background, layout, image, sort, type, args, \
             COALESCE(\"order\", 0), width, height, converted \
             FROM Boards WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], map_board_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn get_buttons_by_board(&self, board_id: i64) -> Result<Vec<ButtonRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, board_id, type, command, title, title_position, title_color, \
             title_box_color, color, icon_color, icon_color2, border_color, shape, icon, \
             img, img2, icon2, color2, shape2, border_color2, title_position2, \
             title_box_color2, title_color2, position, position2, mode, x, y, w, h, options \
             FROM Shortcuts WHERE board_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt
            .query_map([board_id], map_button_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Every shortcut grouped by board id in one query. Whole-board reads
    /// (`list_boards`, the v2 boards build) would otherwise issue one
    /// SELECT per board; global rowid order preserves each board's
    /// per-board `ORDER BY rowid` order.
    pub fn get_buttons_grouped(&self) -> Result<std::collections::HashMap<i64, Vec<ButtonRow>>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, board_id, type, command, title, title_position, title_color, \
             title_box_color, color, icon_color, icon_color2, border_color, shape, icon, \
             img, img2, icon2, color2, shape2, border_color2, title_position2, \
             title_box_color2, title_color2, position, position2, mode, x, y, w, h, options \
             FROM Shortcuts ORDER BY rowid",
        )?;
        let mut grouped: std::collections::HashMap<i64, Vec<ButtonRow>> =
            std::collections::HashMap::new();
        let rows = stmt.query_map([], map_button_row)?;
        for row in rows {
            let button = row?;
            grouped.entry(button.board_id).or_default().push(button);
        }
        Ok(grouped)
    }

    pub fn get_button(&self, id: i64) -> Result<Option<ButtonRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, board_id, type, command, title, title_position, title_color, \
             title_box_color, color, icon_color, icon_color2, border_color, shape, icon, \
             img, img2, icon2, color2, shape2, border_color2, title_position2, \
             title_box_color2, title_color2, position, position2, mode, x, y, w, h, options \
             FROM Shortcuts WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], map_button_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Ids of every shortcut across all boards, for lazy GCs of
    /// per-button side state (the tools store drops entries for deleted
    /// buttons; AUTOINCREMENT ids never get reused).
    pub fn all_button_ids(&self) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM Shortcuts ORDER BY id")?;
        let rows = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Same row as [`Db::get_button`] with the `img`/`img2` columns left
    /// empty. Taps and slider slides read a button per event and must not
    /// materialize multi-MB base64 image strings; every consumer treats an
    /// empty `img` as "no image" (see `build_tile`'s filter).
    pub fn get_button_meta(&self, id: i64) -> Result<Option<ButtonRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, board_id, type, command, title, title_position, title_color, \
             title_box_color, color, icon_color, icon_color2, border_color, shape, icon, \
             '' AS img, '' AS img2, icon2, color2, shape2, border_color2, title_position2, \
             title_box_color2, title_color2, position, position2, mode, x, y, w, h, options \
             FROM Shortcuts WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], map_button_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Open the database for reading and writing, creating an empty
    /// schema when the file does not exist - or when it exists but has
    /// none (an empty or hand-created placeholder must not open into a
    /// database every query fails on). The caller becomes the single
    /// writer - the original desktop app must not have the file open
    /// (docs/decisions.md ADR-001).
    pub fn open_read_write(path: Option<&Path>) -> Result<Db> {
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => default_db_path(),
        };
        if !path.exists() {
            // sqlite creates the file but not its parent directories
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let conn = Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                    | rusqlite::OpenFlags::SQLITE_OPEN_CREATE,
            )?;
            create_schema(&conn)?;
            tracing::info!(path = %path.display(), "created empty pulpitApp database");
            return Ok(Db { conn });
        }
        let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        // A file that merely exists may be empty or junk (torn third-party
        // write): probe the schema instead of trusting the file. A real
        // database answers "tables present" and nothing is written;
        // create_schema's CREATE TABLE IF NOT EXISTS makes the other case
        // idempotent.
        let has_boards: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'Boards')",
            [],
            |r| r.get(0),
        )?;
        if !has_boards {
            create_schema(&conn)?;
            tracing::info!(
                path = %path.display(),
                "existing database file had no schema - created it"
            );
        }
        tracing::info!(path = %path.display(), "opened pulpitApp database (read-write)");
        Ok(Db { conn })
    }

    /// Insert a board; the editor supplies name and grid shape, remaining
    /// columns take the original's defaults. Returns the new id.
    pub fn insert_board(
        &self,
        name: &str,
        background: &str,
        width: i64,
        height: i64,
    ) -> Result<i64> {
        let next_order: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(COALESCE(\"order\", 0)), -1) + 1 FROM Boards",
            [],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO Boards (name, background, width, height, converted, \"order\")
             VALUES (?1, ?2, ?3, ?4, 1, ?5)",
            rusqlite::params![name, background, width, height, next_order],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Insert a fully specified board (`.boardjson` import); `row.id` is
    /// ignored and the new id returned.
    pub fn insert_board_full(&self, row: &BoardRow) -> Result<i64> {
        insert_board_full_on(&self.conn, row)
    }

    /// Run `f` inside a transaction: committed when it returns `Ok`,
    /// rolled back on `Err`. Used by multi-statement writes (imports).
    pub fn with_transaction<T>(&self, f: impl FnOnce(&DbTx<'_>) -> Result<T>) -> Result<T> {
        let tx = self.conn.unchecked_transaction()?;
        let out = f(&DbTx { conn: &tx })?;
        tx.commit()?;
        Ok(out)
    }

    /// Persist every stored column of the board row. A resize runs in one
    /// transaction with a re-clamp of the board's tiles (DESK-03): tiles
    /// left outside the new grid are dropped by the legacy wire's pro
    /// filter and hang off-canvas on the v2 one.
    pub fn update_board(&self, board: &BoardRow) -> Result<()> {
        self.with_transaction(|tx| {
            tx.conn.execute(
                "UPDATE Boards SET name = ?1, background = ?2, layout = ?3, image = ?4, \
                 sort = ?5, type = ?6, args = ?7, \"order\" = ?8, width = ?9, height = ?10, \
                 converted = ?11 WHERE id = ?12",
                rusqlite::params![
                    board.name,
                    board.background,
                    board.layout,
                    board.image,
                    board.sort,
                    board.kind,
                    board.args,
                    board.order,
                    board.width,
                    board.height,
                    board.converted,
                    board.id,
                ],
            )?;
            tx.clamp_board_tiles(board.id, board.width, board.height)
        })
    }

    /// Delete the board together with its shortcuts (the original leaves
    /// orphans behind; we prefer the clean invariant). One transaction, so
    /// a failure mid-way never strands the shortcuts of a live board.
    pub fn delete_board(&self, id: i64) -> Result<()> {
        self.with_transaction(|tx| {
            tx.conn
                .execute("DELETE FROM Shortcuts WHERE board_id = ?1", [id])?;
            tx.conn.execute("DELETE FROM Boards WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    /// Insert a button row; `row.id` is ignored and the new id returned.
    pub fn insert_button(&self, row: &ButtonRow) -> Result<i64> {
        insert_button_on(&self.conn, row)
    }

    /// Persist every stored column of the button row.
    pub fn update_button(&self, row: &ButtonRow) -> Result<()> {
        self.conn.execute(
            "UPDATE Shortcuts SET board_id = ?1, type = ?2, command = ?3, title = ?4, \
             title_position = ?5, title_color = ?6, title_box_color = ?7, color = ?8, \
             icon_color = ?9, icon_color2 = ?10, border_color = ?11, shape = ?12, \
             icon = ?13, img = ?14, img2 = ?15, icon2 = ?16, color2 = ?17, shape2 = ?18, \
             border_color2 = ?19, title_position2 = ?20, title_box_color2 = ?21, \
             title_color2 = ?22, position = ?23, position2 = ?24, mode = ?25, x = ?26, \
             y = ?27, w = ?28, h = ?29, options = ?30 WHERE id = ?31",
            rusqlite::params![
                row.board_id,
                row.kind,
                row.command,
                row.title,
                row.title_position,
                row.title_color,
                row.title_box_color,
                row.color,
                row.icon_color,
                row.icon_color2,
                row.border_color,
                row.shape,
                row.icon,
                row.img,
                row.img2,
                row.icon2,
                row.color2,
                row.shape2,
                row.border_color2,
                row.title_position2,
                row.title_box_color2,
                row.title_color2,
                row.position,
                row.position2,
                row.mode,
                row.x,
                row.y,
                row.w,
                row.h,
                row.options,
                row.id,
            ],
        )?;
        Ok(())
    }

    /// Drag/resize on the editor grid only touches placement.
    pub fn update_button_geometry(&self, id: i64, x: i64, y: i64, w: i64, h: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE Shortcuts SET x = ?1, y = ?2, w = ?3, h = ?4 WHERE id = ?5",
            rusqlite::params![x, y, w, h, id],
        )?;
        Ok(())
    }

    pub fn delete_button(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM Shortcuts WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Remove every shortcut of the board ("Clear board" in the editor).
    pub fn clear_board(&self, board_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM Shortcuts WHERE board_id = ?1", [board_id])?;
        Ok(())
    }
}

/// The original exports store the legacy `sort` column verbatim, where knex
/// returns `''` for the schema default. Accept `""`, numbers and null.
fn de_lenient_int<'de, D>(de: D) -> std::result::Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = <serde_json::Value as serde::Deserialize>::deserialize(de)?;
    Ok(match v {
        serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
        _ => 0,
    })
}

fn map_board_row(row: &rusqlite::Row<'_>) -> std::result::Result<BoardRow, rusqlite::Error> {
    Ok(BoardRow {
        id: row.get(0)?,
        name: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        background: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        layout: row_int(row, 3, 6)?,
        image: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        // legacy `sort` column has a '' default; coerce non-numeric to 0
        sort: row_int(row, 5, 0)?,
        kind: row
            .get::<_, Option<String>>(6)?
            .unwrap_or_else(|| "buttons".into()),
        args: row.get(7)?,
        order: row_int(row, 8, 0)?,
        width: row_int(row, 9, 4)?,
        height: row_int(row, 10, 3)?,
        converted: row_int(row, 11, 1)?,
    })
}

/// Transaction-scoped view of the database: the write methods the import
/// path needs, running on the caller's open transaction.
pub struct DbTx<'a> {
    conn: &'a Connection,
}

impl DbTx<'_> {
    /// Insert a fully specified board; returns the new id.
    pub fn insert_board_full(&self, row: &BoardRow) -> Result<i64> {
        insert_board_full_on(self.conn, row)
    }

    /// Insert a button row; `row.id` is ignored, returns the new id.
    pub fn insert_button(&self, row: &ButtonRow) -> Result<i64> {
        insert_button_on(self.conn, row)
    }

    /// Pull every tile of `board_id` back inside a width*height grid
    /// (DESK-03, [`clamp_placement`]). Rows that already fit are left
    /// untouched, so a rename does not rewrite the whole board.
    pub fn clamp_board_tiles(&self, board_id: i64, width: i64, height: i64) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "SELECT id, COALESCE(x, 0), COALESCE(y, 0), COALESCE(w, 1), COALESCE(h, 1) \
             FROM Shortcuts WHERE board_id = ?1",
        )?;
        let outside: Vec<(i64, i64, i64, i64, i64)> = stmt
            .query_map([board_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .filter_map(|row| {
                let (id, x, y, w, h) = row.ok()?;
                let (cx, cy, cw, ch) = clamp_placement(x, y, w, h, width, height);
                ((cx, cy, cw, ch) != (x, y, w, h)).then_some((id, cx, cy, cw, ch))
            })
            .collect();
        drop(stmt);
        for (id, x, y, w, h) in outside {
            self.conn.execute(
                "UPDATE Shortcuts SET x = ?1, y = ?2, w = ?3, h = ?4 WHERE id = ?5",
                rusqlite::params![x, y, w, h, id],
            )?;
        }
        Ok(())
    }
}

fn insert_board_full_on(conn: &Connection, row: &BoardRow) -> Result<i64> {
    conn.execute(
        "INSERT INTO Boards (name, background, layout, image, sort, type, args, \
         \"order\", width, height, converted) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            row.name,
            row.background,
            row.layout,
            row.image,
            row.sort,
            row.kind,
            row.args,
            row.order,
            row.width,
            row.height,
            row.converted,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn insert_button_on(conn: &Connection, row: &ButtonRow) -> Result<i64> {
    conn.execute(
        "INSERT INTO Shortcuts (board_id, type, command, title, title_position, \
         title_color, title_box_color, color, icon_color, icon_color2, border_color, \
         shape, icon, img, img2, icon2, color2, shape2, border_color2, \
         title_position2, title_box_color2, title_color2, position, position2, \
         mode, x, y, w, h, options) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, \
         ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)",
        rusqlite::params![
            row.board_id,
            row.kind,
            row.command,
            row.title,
            row.title_position,
            row.title_color,
            row.title_box_color,
            row.color,
            row.icon_color,
            row.icon_color2,
            row.border_color,
            row.shape,
            row.icon,
            row.img,
            row.img2,
            row.icon2,
            row.color2,
            row.shape2,
            row.border_color2,
            row.title_position2,
            row.title_box_color2,
            row.title_color2,
            row.position,
            row.position2,
            row.mode,
            row.x,
            row.y,
            row.w,
            row.h,
            row.options,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Read an integer column that legacy databases may store as text
/// (the original schema uses text-affinity columns; "" and "3" both occur).
fn row_int(
    row: &rusqlite::Row<'_>,
    idx: usize,
    default: i64,
) -> std::result::Result<i64, rusqlite::Error> {
    Ok(match row.get::<_, Option<rusqlite::types::Value>>(idx)? {
        Some(rusqlite::types::Value::Integer(n)) => n,
        Some(rusqlite::types::Value::Real(n)) => n as i64,
        Some(rusqlite::types::Value::Text(s)) => s.trim().parse().unwrap_or(default),
        _ => default,
    })
}

/// Same as [`row_int`], but keeps SQL NULL as `None`.
fn row_opt_int(
    row: &rusqlite::Row<'_>,
    idx: usize,
) -> std::result::Result<Option<i64>, rusqlite::Error> {
    Ok(match row.get::<_, Option<rusqlite::types::Value>>(idx)? {
        Some(rusqlite::types::Value::Integer(n)) => Some(n),
        Some(rusqlite::types::Value::Real(n)) => Some(n as i64),
        Some(rusqlite::types::Value::Text(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    })
}

fn map_button_row(row: &rusqlite::Row<'_>) -> std::result::Result<ButtonRow, rusqlite::Error> {
    Ok(ButtonRow {
        id: row.get(0)?,
        board_id: row.get(1)?,
        kind: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        command: row.get(3)?,
        title: row.get(4)?,
        title_position: row_int(row, 5, 0)?,
        title_color: row.get(6)?,
        title_box_color: row.get(7)?,
        color: row.get(8)?,
        icon_color: row.get(9)?,
        icon_color2: row.get(10)?,
        border_color: row.get(11)?,
        shape: row_int(row, 12, 0)?,
        icon: row.get(13)?,
        img: row.get(14)?,
        img2: row.get(15)?,
        icon2: row.get(16)?,
        color2: row.get(17)?,
        shape2: row_int(row, 18, 0)?,
        border_color2: row.get(19)?,
        title_position2: row_int(row, 20, 0)?,
        title_box_color2: row.get(21)?,
        title_color2: row.get(22)?,
        position: row_opt_int(row, 23)?,
        position2: row_int(row, 24, 0)?,
        mode: row
            .get::<_, Option<String>>(25)?
            .unwrap_or_else(|| "button".into()),
        x: row_opt_int(row, 26)?,
        y: row_opt_int(row, 27)?,
        w: row_int(row, 28, 1)?,
        h: row_int(row, 29, 1)?,
        options: row.get(30)?,
    })
}

fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS Boards (
            id integer Primary key AUTOINCREMENT,
            name text,
            background text default '#2c3e50',
            layout integer default 6,
            image text default '',
            sort text default '',
            type text default 'buttons',
            args text default null,
            \"order\" integer default 0,
            width integer default 4,
            height integer default 3,
            converted integer default 1
        );
        CREATE TABLE IF NOT EXISTS Shortcuts (
            id integer primary key autoincrement,
            board_id int,
            title text,
            img text,
            type text,
            command TEXT,
            position int,
            title_position integer default 0,
            title_color text default '#ffffff',
            title_box_color text default '',
            color text default '',
            icon_color text default '',
            icon_color2 text default '',
            border_color text default '',
            shape integer default 0,
            icon text default null,
            img2 text default '',
            icon2 text default null,
            color2 text default '',
            shape2 integer default 0,
            border_color2 text default '',
            title_position2 integer default 0,
            title_box_color2 text default '',
            title_color2 text default '',
            position2 integer default 0,
            mode text default 'button',
            x integer default null,
            y integer default null,
            w integer default 1,
            h integer default 1,
            options text default null
        );",
    )?;
    Ok(())
}

/// Write `bytes` to `path` atomically: the data lands in a sibling
/// `<path>.tmp` file, which is flushed and then renamed over the
/// destination. A crash mid-write leaves the previous file intact
/// instead of a torn or empty one. Used for every JSON config the app
/// rewrites in place (settings.json, editor.json).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let tmp = temp_sibling(path);
    let attempt = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    };
    match attempt() {
        Ok(()) => Ok(()),
        Err(e) => {
            // never leave a stray .tmp behind on failure
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Shared HTTP client constructor: one place for the timeout policy and
/// the user agent every Pulpit HTTP call carries. `status_as_error` picks
/// ureq's default behavior (4xx/5xx become `ureq::Error::Status`) or the
/// tolerant one APIs like Discord's local RPC need (every status comes
/// back as a `Response`).
///
/// Users today: pulpit-discord (OAuth calls), pulpit-aidev (provider
/// limits + Antigravity quota), pulpit-backend (third-party app pings).
/// Skip-sized timeouts on purpose: Discord's local pipe API answers
/// instantly, quota/limits endpoints can be slow. New HTTP consumers -
/// `crates/spotify` is next - must build their agents through this
/// instead of rolling another `Agent::config_builder()` chain.
pub fn http_agent(global_timeout: std::time::Duration, status_as_error: bool) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(global_timeout))
        .http_status_as_error(status_as_error)
        .user_agent(concat!("pulpit/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

/// `<path>.tmp` in the same directory, so the rename stays on one volume.
fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

/// `~/pulpitApp/database.db`; on first use this is a copy of the original
/// Deckboard app's database (see [`data_dir`]).
pub fn default_db_path() -> PathBuf {
    data_dir().join("database.db")
}

/// Data directory root: `~/pulpitApp`. The first call copies a legacy
/// `~/deckboard` directory (the original Deckboard app's data) into it, so
/// upgrading needs no manual steps and the original app keeps its files.
pub fn data_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let target = home.join("pulpitApp");
    migrate_legacy_data(&home.join("deckboard"), &target);
    target
}

const LEGACY_FILES: &[&str] = &[
    "database.db",
    "settings.json",
    "editor.json",
    "devices.json",
];
const LEGACY_DIRS: &[&str] = &["extensions", "assets"];

/// One-time, best-effort copy from the original Deckboard data directory.
/// The full copy is staged in a sibling `<target>.migrating` directory and
/// renamed into place in one step, so a crash mid-copy never leaves a
/// half-populated `~/pulpitApp`: the next run per-item skips what already
/// landed in the staging dir and finishes the job (ADR-011). `logs/` is
/// intentionally not carried over.
fn migrate_legacy_data(legacy: &Path, target: &Path) {
    if target.exists() || !legacy.is_dir() {
        return;
    }
    let staging = migrating_sibling(target);
    if let Err(e) = std::fs::create_dir_all(&staging) {
        tracing::warn!(
            error = %e,
            "cannot create migration staging directory, skipping legacy migration"
        );
        return;
    }
    let mut copied: Vec<&str> = Vec::new();
    for name in LEGACY_FILES
        .iter()
        .copied()
        .chain(LEGACY_DIRS.iter().copied())
    {
        let from = legacy.join(name);
        let to = staging.join(name);
        if to.exists() {
            continue; // landed before a previous crash; resume
        }
        let copied_ok = if from.is_dir() {
            copy_tree(&from, &to).is_ok()
        } else if from.is_file() {
            std::fs::copy(&from, &to).is_ok()
        } else {
            false
        };
        if copied_ok {
            copied.push(name);
        } else if from.exists() {
            // the item stays missing but must not block the rename forever
            tracing::warn!(item = name, "legacy migration could not copy item");
        }
    }
    copy_db_sidecars(legacy, &staging);
    // either the rename lands the whole directory at once, or the target
    // stays absent and the next run resumes from the staging dir
    match std::fs::rename(&staging, target) {
        Ok(()) => {
            tracing::info!(
                from = %legacy.display(),
                to = %target.display(),
                migrated = ?copied,
                "migrated data from the original Deckboard app"
            );
        }
        Err(e) => tracing::warn!(
            error = %e,
            "cannot finalize legacy migration; it will resume on the next start"
        ),
    }
}

/// `database.db` may carry un-checkpointed commits in its WAL sidecars;
/// copying them keeps recent writes from the original app (best-effort,
/// skipped when already present so a resumed run stays idempotent).
fn copy_db_sidecars(legacy: &Path, staging: &Path) {
    for suffix in ["-wal", "-shm"] {
        let from = legacy.join(format!("database.db{suffix}"));
        let to = staging.join(format!("database.db{suffix}"));
        if from.is_file() && !to.exists() {
            if let Err(e) = std::fs::copy(&from, &to) {
                tracing::warn!(error = %e, sidecar = suffix, "could not copy database sidecar");
            }
        }
    }
}

/// Sibling directory the migration stages into before the final rename.
fn migrating_sibling(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_default();
    name.push(".migrating");
    target.with_file_name(name)
}

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_migration_copies_data_but_not_logs() {
        let home = tempfile::tempdir().unwrap();
        let legacy = home.path().join("deckboard");
        let target = home.path().join("pulpitApp");
        std::fs::create_dir_all(legacy.join("extensions/pkg")).unwrap();
        std::fs::create_dir_all(legacy.join("logs")).unwrap();
        std::fs::write(legacy.join("database.db"), b"db").unwrap();
        std::fs::write(legacy.join("settings.json"), b"{}").unwrap();
        std::fs::write(legacy.join("editor.json"), b"{}").unwrap();
        std::fs::write(legacy.join("devices.json"), b"[]").unwrap();
        std::fs::write(legacy.join("extensions/pkg/index.js"), b"module.exports").unwrap();
        std::fs::write(legacy.join("logs/old.log"), b"x").unwrap();

        migrate_legacy_data(&legacy, &target);

        assert_eq!(std::fs::read(target.join("database.db")).unwrap(), b"db");
        assert_eq!(
            std::fs::read(target.join("extensions/pkg/index.js")).unwrap(),
            b"module.exports"
        );
        assert!(!target.join("logs").exists(), "logs are not carried over");

        // Second run is a no-op: newer data in the target must survive.
        std::fs::write(target.join("database.db"), b"newer").unwrap();
        migrate_legacy_data(&legacy, &target);
        assert_eq!(std::fs::read(target.join("database.db")).unwrap(), b"newer");
    }

    #[test]
    fn legacy_migration_skips_when_no_legacy_dir() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("pulpitApp");
        migrate_legacy_data(&home.path().join("deckboard"), &target);
        assert!(!target.exists(), "nothing to migrate, nothing created");
    }

    #[test]
    fn legacy_migration_resumes_from_a_crashed_partial_copy() {
        let home = tempfile::tempdir().unwrap();
        let legacy = home.path().join("deckboard");
        let target = home.path().join("pulpitApp");
        let staging = home.path().join("pulpitApp.migrating");
        std::fs::create_dir_all(legacy.join("extensions/pkg")).unwrap();
        std::fs::write(legacy.join("database.db"), b"db").unwrap();
        std::fs::write(legacy.join("settings.json"), b"{}").unwrap();
        std::fs::write(legacy.join("editor.json"), b"{}").unwrap();
        std::fs::write(legacy.join("devices.json"), b"[]").unwrap();
        std::fs::write(legacy.join("extensions/pkg/index.js"), b"module.exports").unwrap();

        // simulate a crash after only database.db landed: the staging dir
        // exists, half-populated, and the target was never created
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::copy(legacy.join("database.db"), staging.join("database.db")).unwrap();

        migrate_legacy_data(&legacy, &target);

        // the rerun resumes per-item and completes the migration
        assert_eq!(std::fs::read(target.join("database.db")).unwrap(), b"db");
        assert_eq!(std::fs::read(target.join("settings.json")).unwrap(), b"{}");
        assert_eq!(
            std::fs::read(target.join("extensions/pkg/index.js")).unwrap(),
            b"module.exports"
        );
        assert!(
            !staging.exists(),
            "the staging dir is renamed away, not kept"
        );
    }

    #[test]
    fn legacy_migration_carries_database_wal_sidecars() {
        let home = tempfile::tempdir().unwrap();
        let legacy = home.path().join("deckboard");
        let target = home.path().join("pulpitApp");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("database.db"), b"db").unwrap();
        // an un-checkpointed original app leaves its recent commits here
        std::fs::write(legacy.join("database.db-wal"), b"wal").unwrap();
        std::fs::write(legacy.join("database.db-shm"), b"shm").unwrap();

        migrate_legacy_data(&legacy, &target);

        assert_eq!(
            std::fs::read(target.join("database.db-wal")).unwrap(),
            b"wal"
        );
        assert_eq!(
            std::fs::read(target.join("database.db-shm")).unwrap(),
            b"shm"
        );
    }

    #[test]
    fn schema_and_queries_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_or_create(&dir.path().join("t.db")).unwrap();
        let conn_note = db.conn.execute(
            "INSERT INTO Boards (name, background, width, height) VALUES ('My Board', '#2c3e50', 4, 3)",
            [],
        );
        assert!(conn_note.is_ok());
        let boards = db.get_boards().unwrap();
        assert_eq!(boards.len(), 1);
        assert_eq!(boards[0].name, "My Board");

        db.conn
            .execute(
                "INSERT INTO Shortcuts (board_id, type, command, title, x, y, w, h)
                 VALUES (1, 'url', 'https://example.com', 'Example', 0, 0, 1, 1)",
                [],
            )
            .unwrap();
        let buttons = db.get_buttons_by_board(1).unwrap();
        assert_eq!(buttons.len(), 1);
        assert_eq!(buttons[0].kind, "url");
        let by_id = db.get_button(1).unwrap().unwrap();
        assert_eq!(by_id.command.as_deref(), Some("https://example.com"));
        assert!(db.get_button(999).unwrap().is_none());
    }

    #[test]
    fn missing_file_is_reported() {
        let err = Db::open_read_only(Some(Path::new("Z:/nope/pulpit.db"))).unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    fn sample_button(board_id: i64, x: i64, y: i64) -> ButtonRow {
        ButtonRow {
            id: 0,
            board_id,
            kind: "url".into(),
            command: Some("https://example.com".into()),
            title: Some("Example".into()),
            title_position: 1,
            title_color: Some("#ffffff".into()),
            title_box_color: Some("#000000".into()),
            color: Some("#8e44ad".into()),
            icon_color: Some("#ffffff".into()),
            icon_color2: None,
            border_color: None,
            shape: 1,
            icon: Some("fas link".into()),
            img: Some("data:image/png;base64,AAA".into()),
            img2: None,
            icon2: None,
            color2: None,
            shape2: 0,
            border_color2: None,
            title_position2: 0,
            title_box_color2: None,
            title_color2: None,
            position: None,
            position2: 0,
            mode: "button".into(),
            x: Some(x),
            y: Some(y),
            w: 2,
            h: 1,
            options: Some("--flag".into()),
        }
    }

    #[test]
    fn board_crud_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_or_create(&dir.path().join("t.db")).unwrap();

        let a = db.insert_board("First", "#2c3e50", 4, 3).unwrap();
        let b = db.insert_board("Second", "#2c3e50", 4, 3).unwrap();
        assert_ne!(a, b);

        // new boards get the original's defaults and ascending order
        let boards = db.get_boards().unwrap();
        assert_eq!(boards.len(), 2);
        assert_eq!(boards[0].background, "#2c3e50");
        assert_eq!((boards[0].width, boards[0].height), (4, 3));
        assert_eq!(boards[1].order, boards[0].order + 1);

        // full update persists every column
        let mut board = boards[0].clone();
        board.name = "Renamed".into();
        board.background = "#1a1a2e".into();
        board.width = 7;
        board.height = 5;
        board.image = "data:image/png;base64,AAA".into();
        db.update_board(&board).unwrap();
        let reloaded = &db.get_boards().unwrap()[0];
        assert_eq!(reloaded.name, "Renamed");
        assert_eq!(reloaded.background, "#1a1a2e");
        assert_eq!((reloaded.width, reloaded.height), (7, 5));
        assert_eq!(reloaded.image, "data:image/png;base64,AAA");

        // delete cascades to the board's shortcuts
        db.insert_button(&sample_button(a, 0, 0)).unwrap();
        db.delete_board(a).unwrap();
        assert!(db.get_boards().unwrap().iter().all(|b| b.id != a));
        assert!(db.get_buttons_by_board(a).unwrap().is_empty());
    }

    #[test]
    fn update_board_clamps_tiles_to_the_shrunk_grid() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_or_create(&dir.path().join("t.db")).unwrap();
        let board = db.insert_board("B", "#2c3e50", 8, 8).unwrap();
        let fits = sample_button(board, 1, 1);
        let mut hangs = sample_button(board, 5, 3);
        hangs.w = 2;
        let mut huge = sample_button(board, 2, 2);
        huge.w = 20;
        huge.h = 2;
        let fits_id = db.insert_button(&fits).unwrap();
        let hangs_id = db.insert_button(&hangs).unwrap();
        let huge_id = db.insert_button(&huge).unwrap();

        let mut shrunk = db.get_board(board).unwrap().unwrap();
        shrunk.width = 4;
        shrunk.height = 3;
        db.update_board(&shrunk).unwrap();

        let tiles = db.get_buttons_by_board(board).unwrap();
        let by_id = |id: i64| tiles.iter().find(|t| t.id == id).unwrap();
        let place = |t: &ButtonRow| (t.x, t.y, t.w, t.h);
        // a tile that already fits is left untouched
        assert_eq!(place(by_id(fits_id)), (Some(1), Some(1), 2, 1));
        // origin pulled back inside (x <= W-w, y <= H-h)
        assert_eq!(place(by_id(hangs_id)), (Some(2), Some(2), 2, 1));
        // a tile larger than the grid shrinks to it
        assert_eq!(place(by_id(huge_id)), (Some(0), Some(1), 4, 2));
    }

    #[test]
    fn delete_board_is_atomic_when_the_board_row_delete_fails() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_or_create(&dir.path().join("t.db")).unwrap();
        let a = db.insert_board("A", "#2c3e50", 4, 3).unwrap();
        let b = db.insert_board("B", "#2c3e50", 4, 3).unwrap();
        db.insert_button(&sample_button(a, 0, 0)).unwrap();
        db.insert_button(&sample_button(a, 1, 0)).unwrap();
        db.insert_button(&sample_button(b, 0, 0)).unwrap();

        // veto the Boards delete only: if the two statements are not one
        // transaction, the Shortcuts delete stays committed (orphaned rows)
        db.conn
            .execute(
                "CREATE TRIGGER veto_board_delete BEFORE DELETE ON Boards
                 BEGIN SELECT RAISE(ABORT, 'veto'); END",
                [],
            )
            .unwrap();
        let result = db.delete_board(a);
        db.conn
            .execute("DROP TRIGGER veto_board_delete", [])
            .unwrap();

        assert!(result.is_err(), "the vetoed delete must fail");
        assert!(
            db.get_boards().unwrap().iter().any(|row| row.id == a),
            "the board row must survive the failed delete"
        );
        assert_eq!(
            db.get_buttons_by_board(a).unwrap().len(),
            2,
            "its shortcuts must be restored with it, not orphaned"
        );
        assert_eq!(db.get_buttons_by_board(b).unwrap().len(), 1);
    }

    #[test]
    fn button_crud_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_or_create(&dir.path().join("t.db")).unwrap();
        let board = db.insert_board("Board", "#2c3e50", 4, 3).unwrap();

        let id = db.insert_button(&sample_button(board, 1, 2)).unwrap();
        let stored = db.get_button(id).unwrap().unwrap();
        assert_eq!(stored.board_id, board);
        assert_eq!(stored.kind, "url");
        assert_eq!(stored.title.as_deref(), Some("Example"));
        assert_eq!((stored.shape, stored.title_position), (1, 1));
        assert_eq!(stored.icon.as_deref(), Some("fas link"));
        assert_eq!(stored.options.as_deref(), Some("--flag"));
        assert_eq!(
            (stored.x.unwrap(), stored.y.unwrap(), stored.w, stored.h),
            (1, 2, 2, 1)
        );

        // full update keeps unedited columns intact
        let mut edited = stored.clone();
        edited.kind = "key".into();
        edited.command = Some("CTRL + K".into());
        edited.color2 = Some("#c0392b".into());
        db.update_button(&edited).unwrap();
        let reloaded = db.get_button(id).unwrap().unwrap();
        assert_eq!(reloaded.kind, "key");
        assert_eq!(reloaded.color2.as_deref(), Some("#c0392b"));
        assert_eq!(reloaded.icon.as_deref(), Some("fas link"));

        // geometry-only update (drag/resize)
        db.update_button_geometry(id, 3, 1, 2, 2).unwrap();
        let moved = db.get_button(id).unwrap().unwrap();
        assert_eq!(
            (moved.x.unwrap(), moved.y.unwrap(), moved.w, moved.h),
            (3, 1, 2, 2)
        );

        db.delete_button(id).unwrap();
        assert!(db.get_button(id).unwrap().is_none());
    }

    #[test]
    fn button_meta_matches_get_button_except_images() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_or_create(&dir.path().join("t.db")).unwrap();
        let board = db.insert_board("Board", "#2c3e50", 4, 3).unwrap();

        let mut row = sample_button(board, 0, 0);
        row.img2 = Some("data:image/png;base64,BBB".into());
        let id = db.insert_button(&row).unwrap();

        // the meta read skips the image columns; every other field must be
        // identical to the full read (interaction lookups rely on it)
        let full = db.get_button(id).unwrap().unwrap();
        let meta = db.get_button_meta(id).unwrap().unwrap();
        assert_eq!(meta.img.as_deref(), Some(""));
        assert_eq!(meta.img2.as_deref(), Some(""));
        assert_eq!(meta.id, full.id);
        assert_eq!(meta.board_id, full.board_id);
        assert_eq!(meta.kind, full.kind);
        assert_eq!(meta.command, full.command);
        assert_eq!(meta.title, full.title);
        assert_eq!(meta.mode, full.mode);
        assert_eq!(meta.options, full.options);
        assert_eq!(meta.x, full.x);
        assert_eq!(meta.y, full.y);
        assert_eq!(meta.w, full.w);
        assert_eq!(meta.h, full.h);
        assert_eq!(meta.color, full.color);
        assert_eq!(meta.color2, full.color2);
        assert_eq!(meta.icon, full.icon);
        assert_eq!(meta.icon2, full.icon2);
        assert_eq!(meta.shape, full.shape);
        assert_eq!(meta.shape2, full.shape2);
        assert_eq!(meta.title_position, full.title_position);
        assert_eq!(meta.title_position2, full.title_position2);
        assert!(db.get_button_meta(999).unwrap().is_none());
    }

    #[test]
    fn lenient_board_columns_accept_legacy_text_junk() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_or_create(&dir.path().join("t.db")).unwrap();

        // legacy boards carry '' in integer columns (knex defaults); a
        // strict read fails the whole SELECT and empties the board list
        db.conn
            .execute(
                "INSERT INTO Boards (name, background, layout, image, sort, type, \"order\", \
                     width, height, converted)
                 VALUES ('Junk', '#2c3e50', '', '', '7', 'buttons', '', '', '', '')",
                [],
            )
            .unwrap();
        db.conn
            .execute("INSERT INTO Boards (name) VALUES ('Clean')", [])
            .unwrap();

        let boards = db.get_boards().unwrap();
        assert_eq!(boards.len(), 2, "one bad row must not empty the list");
        let junk = boards.iter().find(|b| b.name == "Junk").unwrap();
        assert_eq!(junk.layout, 6);
        assert_eq!((junk.width, junk.height), (4, 3));
        assert_eq!(junk.order, 0);
        assert_eq!(junk.converted, 1);
        assert_eq!(junk.sort, 7, "numeric text parses, like the Shortcuts path");

        // single-board reads are lenient the same way
        let by_id = db.get_board(junk.id).unwrap().unwrap();
        assert_eq!(by_id.layout, 6);
        assert_eq!((by_id.width, by_id.height), (4, 3));
    }

    #[test]
    fn clear_board_removes_only_that_boards_buttons() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_or_create(&dir.path().join("t.db")).unwrap();
        let board_a = db.insert_board("A", "#2c3e50", 4, 3).unwrap();
        let board_b = db.insert_board("B", "#2c3e50", 4, 3).unwrap();
        db.insert_button(&sample_button(board_a, 0, 0)).unwrap();
        db.insert_button(&sample_button(board_a, 1, 0)).unwrap();
        db.insert_button(&sample_button(board_b, 0, 0)).unwrap();

        db.clear_board(board_a).unwrap();
        assert!(db.get_buttons_by_board(board_a).unwrap().is_empty());
        assert_eq!(db.get_buttons_by_board(board_b).unwrap().len(), 1);
    }

    #[test]
    fn grouped_buttons_match_per_board_reads() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_or_create(&dir.path().join("t.db")).unwrap();
        let board_a = db.insert_board("A", "#2c3e50", 4, 3).unwrap();
        let board_b = db.insert_board("B", "#2c3e50", 4, 3).unwrap();
        for (board, x) in [(board_a, 0), (board_a, 1), (board_b, 0)] {
            db.insert_button(&sample_button(board, x, 0)).unwrap();
        }

        let grouped = db.get_buttons_grouped().unwrap();
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[&board_a].len(), 2);
        assert_eq!(grouped[&board_b].len(), 1);
        // ids and order must match the per-board query exactly
        for board in [board_a, board_b] {
            let per_board: Vec<_> = db
                .get_buttons_by_board(board)
                .unwrap()
                .into_iter()
                .map(|b| b.id)
                .collect();
            let grouped_ids: Vec<_> = grouped[&board].iter().map(|b| b.id).collect();
            assert_eq!(per_board, grouped_ids);
        }
        // a board without shortcuts has no entry at all
        db.delete_button(grouped[&board_a][0].id).unwrap();
        db.delete_button(grouped[&board_a][1].id).unwrap();
        assert!(!db.get_buttons_grouped().unwrap().contains_key(&board_a));
    }

    #[test]
    fn open_read_write_creates_missing_database_with_schema() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("fresh/pulpitApp");
        let path = nested.join("database.db");

        // a clean install has no legacy copy to migrate; the writer must
        // still get a database instead of a NotFound dead end
        let db = Db::open_read_write(Some(&path)).unwrap();

        assert!(path.exists(), "the database file is created");
        assert!(db.get_boards().unwrap().is_empty());
        let id = db.insert_board("First", "#2c3e50", 4, 3).unwrap();
        assert!(db.get_board(id).unwrap().is_some());

        // reopening never clobbers what is already there
        drop(db);
        let db = Db::open_read_write(Some(&path)).unwrap();
        assert_eq!(db.get_boards().unwrap().len(), 1);
    }

    #[test]
    fn open_read_write_creates_schema_for_an_empty_existing_file() {
        // the file merely existing must not count as "schema present": a
        // 0-byte placeholder (or torn write) used to open "successfully"
        // and then fail every query with "no such table"
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("database.db");
        std::fs::write(&path, b"").unwrap();

        let db = Db::open_read_write(Some(&path)).unwrap();
        assert!(db.get_boards().unwrap().is_empty());
        let id = db.insert_board("First", "#2c3e50", 4, 3).unwrap();
        assert!(db.get_board(id).unwrap().is_some());
    }

    #[test]
    fn open_read_write_rejects_a_non_sqlite_file() {
        // junk bytes are not rescuable: say so at open instead of handing
        // back a database whose every query fails with "not a database"
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("database.db");
        std::fs::write(&path, b"this is not sqlite at all").unwrap();

        let err = Db::open_read_write(Some(&path)).unwrap_err();
        assert!(matches!(err, DbError::Sqlite(_)), "got {err:?}");
    }

    #[test]
    fn open_read_write_still_errors_on_an_unwritable_path() {
        // creation cannot rescue a path whose parent cannot exist
        let err = Db::open_read_write(Some(Path::new("Z:/nope/pulpit.db"))).unwrap_err();
        assert!(matches!(err, DbError::Sqlite(_)));
    }

    #[test]
    fn write_atomic_replaces_content_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"previous").unwrap();

        write_atomic(&path, b"fresh bytes").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"fresh bytes");
        assert!(
            !dir.path().join("settings.json.tmp").exists(),
            "the temp sibling must be renamed away, not left behind"
        );

        // a path with no existing file is created directly
        let fresh = dir.path().join("editor.json");
        write_atomic(&fresh, b"{}").unwrap();
        assert_eq!(std::fs::read(&fresh).unwrap(), b"{}");
    }

    #[test]
    fn lenient_int_columns_accept_legacy_text_junk() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_or_create(&dir.path().join("t.db")).unwrap();
        let board = db.insert_board("Legacy", "#2c3e50", 4, 3).unwrap();

        // legacy databases carry '' and text in int-affinity columns; a
        // strict read blanks the whole board, so reads must fall back
        db.conn
            .execute(
                "INSERT INTO Shortcuts (board_id, type, command, title, title_position, shape,
                     position, position2, x, y, w, h)
                 VALUES (?1, 'url', 'https://example.com', 'Junk', '3', '', '', '2', '', '1', '2', '1')",
                rusqlite::params![board],
            )
            .unwrap();
        let buttons = db.get_buttons_by_board(board).unwrap();
        assert_eq!(buttons.len(), 1);
        let b = &buttons[0];
        assert_eq!(b.title_position, 3);
        assert_eq!((b.shape, b.w, b.h), (0, 2, 1));
        assert_eq!((b.x, b.y), (None, Some(1)));
        assert_eq!(b.position, None);
        assert_eq!(b.position2, 2);
    }

    #[test]
    fn http_agent_carries_the_pulpit_user_agent() {
        // one shared constructor means one user agent: assert it survives
        // onto the wire so the policy stays in this one place
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut buf = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = std::io::Read::read(&mut sock, &mut buf).unwrap();
                if n == 0 {
                    break;
                }
                head.extend_from_slice(&buf[..n]);
            }
            let _ = std::io::Write::write_all(
                &mut sock,
                b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx",
            );
            String::from_utf8_lossy(&head).into_owned()
        });
        let mut resp = http_agent(std::time::Duration::from_secs(10), true)
            .get(&format!("http://{addr}/ua"))
            .call()
            .unwrap();
        let _ = resp.body_mut().read_to_string();
        let head = server.join().unwrap();
        let header = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("user-agent:"))
            .expect("user agent header present");
        let (name, value) = header.split_once(':').unwrap();
        assert_eq!(name.to_ascii_lowercase(), "user-agent");
        assert_eq!(value.trim(), format!("pulpit/{}", env!("CARGO_PKG_VERSION")));
    }
}
