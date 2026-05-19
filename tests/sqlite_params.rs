//! sqlite 参数宏的外部 crate 使用回归测试。

use easy_rust::sqlite;

#[test]
fn params_macro_is_usable_from_external_crate() -> sqlite::Result<()> {
    let db = sqlite::memory()?;

    db.execute("CREATE TABLE users (id INTEGER, name TEXT)")?;
    db.execute_params(
        "INSERT INTO users (id, name) VALUES (?1, ?2)",
        sqlite::params![1, "Ada"],
    )?;

    let row = db
        .get_params("SELECT name FROM users WHERE id = ?1", sqlite::params![1])?
        .ok_or_else(|| sqlite::ErrorKind::Shape {
            operation: "get_params",
            sql: "SELECT name FROM users WHERE id = ?1".to_owned(),
            message: "missing row".to_owned(),
        })?;

    assert_eq!(row.text("name")?, Some("Ada"));
    Ok(())
}
