//! SQLite access for the Deckboard database (`~/deckboard/database.db`).
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
    /// Open the database read-only. Pass `None` to use `~/deckboard/database.db`.
    pub fn open_read_only(path: Option<&Path>) -> Result<Db> {
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => default_db_path(),
        };
        if !path.exists() {
            return Err(DbError::NotFound(path));
        }
        let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        tracing::info!(path = %path.display(), "opened deckboard database (read-only)");
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

    /// Open the database for reading and writing. The caller becomes the
    /// single writer - the original desktop app must not have the file open
    /// (docs/decisions.md ADR-001).
    pub fn open_read_write(path: Option<&Path>) -> Result<Db> {
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => default_db_path(),
        };
        if !path.exists() {
            return Err(DbError::NotFound(path));
        }
        let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        tracing::info!(path = %path.display(), "opened deckboard database (read-write)");
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

    /// Persist every stored column of the board row.
    pub fn update_board(&self, board: &BoardRow) -> Result<()> {
        self.conn.execute(
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
        Ok(())
    }

    /// Delete the board together with its shortcuts (the original leaves
    /// orphans behind; we prefer the clean invariant).
    pub fn delete_board(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM Shortcuts WHERE board_id = ?1", [id])?;
        self.conn
            .execute("DELETE FROM Boards WHERE id = ?1", [id])?;
        Ok(())
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
        layout: row.get::<_, Option<i64>>(3)?.unwrap_or(6),
        image: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        // legacy `sort` column has a '' default; coerce non-numeric to 0
        sort: row.get::<_, Option<i64>>(5).unwrap_or(None).unwrap_or(0),
        kind: row
            .get::<_, Option<String>>(6)?
            .unwrap_or_else(|| "buttons".into()),
        args: row.get(7)?,
        order: row.get(8)?,
        width: row.get::<_, Option<i64>>(9)?.unwrap_or(4),
        height: row.get::<_, Option<i64>>(10)?.unwrap_or(3),
        converted: row.get::<_, Option<i64>>(11)?.unwrap_or(1),
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

/// `~/deckboard/database.db` - the same file the original desktop app uses.
pub fn default_db_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join("deckboard").join("database.db")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let err = Db::open_read_only(Some(Path::new("Z:/nope/deckboard.db"))).unwrap_err();
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
    fn open_read_write_rejects_missing_file() {
        let err = Db::open_read_write(Some(Path::new("Z:/nope/deckboard.db"))).unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
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
}
