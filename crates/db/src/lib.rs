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
}

pub type Result<T> = std::result::Result<T, DbError>;

/// Row of the `Boards` table.
#[derive(Debug, Clone)]
pub struct BoardRow {
    pub id: i64,
    pub name: String,
    pub background: String,
    pub layout: i64,
    pub image: String,
    pub sort: i64,
    pub kind: String,
    pub args: Option<String>,
    pub order: i64,
    pub width: i64,
    pub height: i64,
    pub converted: i64,
}

/// Row of the `Shortcuts` table (one macro button/slider/wheel).
#[derive(Debug, Clone)]
pub struct ButtonRow {
    pub id: i64,
    pub board_id: i64,
    pub kind: String,
    pub command: Option<String>,
    pub title: Option<String>,
    pub title_position: i64,
    pub title_color: Option<String>,
    pub title_box_color: Option<String>,
    pub color: Option<String>,
    pub icon_color: Option<String>,
    pub icon_color2: Option<String>,
    pub border_color: Option<String>,
    pub shape: i64,
    pub icon: Option<String>,
    pub img: Option<String>,
    pub img2: Option<String>,
    pub icon2: Option<String>,
    pub color2: Option<String>,
    pub shape2: i64,
    pub border_color2: Option<String>,
    pub title_position2: i64,
    pub title_box_color2: Option<String>,
    pub title_color2: Option<String>,
    pub position: Option<i64>,
    pub position2: i64,
    pub mode: String,
    pub x: Option<i64>,
    pub y: Option<i64>,
    pub w: i64,
    pub h: i64,
    pub options: Option<String>,
}

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
        let conn = Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
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
        let mut stmt = self.conn.prepare(
            "SELECT id, name, background, layout, image, sort, type, args, \
             COALESCE(\"order\", 0), width, height, converted \
             FROM Boards ORDER BY COALESCE(\"order\", 0), id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(BoardRow {
                    id: row.get(0)?,
                    name: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    background: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    layout: row.get::<_, Option<i64>>(3)?.unwrap_or(6),
                    image: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    // legacy `sort` column has a '' default; coerce non-numeric to 0
                    sort: row.get::<_, Option<i64>>(5).unwrap_or(None).unwrap_or(0),
                    kind: row.get::<_, Option<String>>(6)?.unwrap_or_else(|| "buttons".into()),
                    args: row.get(7)?,
                    order: row.get(8)?,
                    width: row.get::<_, Option<i64>>(9)?.unwrap_or(4),
                    height: row.get::<_, Option<i64>>(10)?.unwrap_or(3),
                    converted: row.get::<_, Option<i64>>(11)?.unwrap_or(1),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
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
}

fn map_button_row(
    row: &rusqlite::Row<'_>,
) -> std::result::Result<ButtonRow, rusqlite::Error> {
    Ok(ButtonRow {
        id: row.get(0)?,
        board_id: row.get(1)?,
        kind: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        command: row.get(3)?,
        title: row.get(4)?,
        title_position: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
        title_color: row.get(6)?,
        title_box_color: row.get(7)?,
        color: row.get(8)?,
        icon_color: row.get(9)?,
        icon_color2: row.get(10)?,
        border_color: row.get(11)?,
        shape: row.get::<_, Option<i64>>(12)?.unwrap_or(0),
        icon: row.get(13)?,
        img: row.get(14)?,
        img2: row.get(15)?,
        icon2: row.get(16)?,
        color2: row.get(17)?,
        shape2: row.get::<_, Option<i64>>(18)?.unwrap_or(0),
        border_color2: row.get(19)?,
        title_position2: row.get::<_, Option<i64>>(20)?.unwrap_or(0),
        title_box_color2: row.get(21)?,
        title_color2: row.get(22)?,
        position: row.get(23)?,
        position2: row.get::<_, Option<i64>>(24)?.unwrap_or(0),
        mode: row.get::<_, Option<String>>(25)?.unwrap_or_else(|| "button".into()),
        x: row.get(26)?,
        y: row.get(27)?,
        w: row.get::<_, Option<i64>>(28)?.unwrap_or(1),
        h: row.get::<_, Option<i64>>(29)?.unwrap_or(1),
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
}
