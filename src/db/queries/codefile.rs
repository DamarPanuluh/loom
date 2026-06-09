//! CodeFile node queries.

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::CodeFile;

use super::row::{col_map, get, str_val};

pub fn insert_codefile(db: &dyn LoomDb, cf: &CodeFile) -> Result<()> {
    let q = format!(
        "INSERT (:CodeFile {{id: '{id}', path: '{path}', language: '{lang}', \
         last_modified: '{mtime}'}})",
        id    = esc(&cf.id),
        path  = esc(&cf.path),
        lang  = esc(&cf.language),
        mtime = esc(&cf.last_modified),
    );
    db.execute(&q)?;
    Ok(())
}

pub fn list_codefiles(db: &dyn LoomDb) -> Result<Vec<CodeFile>> {
    let q = "MATCH (cf:CodeFile) \
             RETURN cf.id, cf.path, cf.language, cf.last_modified \
             ORDER BY cf.path";
    let result = db.execute(q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_codefile(row, &cols)).collect())
}

pub fn update_codefile_mtime(db: &dyn LoomDb, id: &str, mtime: &str) -> Result<bool> {
    let check = db.execute(&format!(
        "MATCH (cf:CodeFile {{id: '{}'}}) RETURN cf.id", esc(id)
    ))?;
    if check.rows().is_empty() {
        return Ok(false);
    }
    db.execute(&format!(
        "MATCH (cf:CodeFile {{id: '{}'}}) SET cf.last_modified = '{}'",
        esc(id), esc(mtime)
    ))?;
    Ok(true)
}

fn row_to_codefile(row: &[Value], cols: &HashMap<&str, usize>) -> CodeFile {
    CodeFile {
        id:            str_val(get(row, cols, "cf.id")),
        path:          str_val(get(row, cols, "cf.path")),
        language:      str_val(get(row, cols, "cf.language")),
        last_modified: str_val(get(row, cols, "cf.last_modified")),
    }
}
