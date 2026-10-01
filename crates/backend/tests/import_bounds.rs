//! `import_boards` hardening (audit item C4): a hostile or corrupt
//! `.boardjson` must be rejected up front, before anything is inserted.

use pulpit_backend::SqlBackend;
use pulpit_db::Db;
use pulpit_legacy::service::Backend;
use serde_json::{json, Value};

fn backend() -> SqlBackend {
    SqlBackend::new(Db::open_or_create(std::path::Path::new(":memory:")).unwrap())
}

fn board(width: Value, height: Value, macros: usize) -> Value {
    json!({
        "name": "Probe",
        "background": "#2c3e50",
        "width": width,
        "height": height,
        "macros": vec![json!({ "type": "url", "command": "https://example.com" }); macros],
    })
}

#[test]
fn oversized_dimensions_are_rejected_before_anything_is_imported() {
    let backend = backend();
    let err = backend
        .import_boards(&[board(json!(1_000_000), json!(1_000_000), 0)])
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("dimension"),
        "got: {err}"
    );
    assert!(
        backend.get_boards().is_empty(),
        "a rejected import must not leave partial state"
    );
}

#[test]
fn non_integer_dimensions_are_rejected() {
    let backend = backend();
    let err = backend
        .import_boards(&[board(json!("32"), json!(3), 0)])
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("bad board entry"),
        "got: {err}"
    );
}

#[test]
fn a_macros_flood_is_rejected() {
    let backend = backend();
    let err = backend.import_boards(&[board(json!(32), json!(32), 2000)]).unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("too many"),
        "got: {err}"
    );
    assert!(backend.get_boards().is_empty());
}

#[test]
fn a_boards_flood_is_rejected() {
    let backend = backend();
    let flood: Vec<Value> = (0..101).map(|_| board(json!(4), json!(3), 1)).collect();
    let err = backend.import_boards(&flood).unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("too many"),
        "got: {err}"
    );
}

#[test]
fn a_full_legal_board_still_imports() {
    let backend = backend();
    let ids = backend
        .import_boards(&[board(json!(32), json!(32), 3)])
        .unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(backend.get_boards().len(), 1);
    assert_eq!(backend.get_buttons_by_board(ids[0]).len(), 3);
}
