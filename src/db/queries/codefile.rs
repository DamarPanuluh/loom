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
         last_modified: '{mtime}', imports: '{imports}', content_hash: '{hash}'}})",
        id      = esc(&cf.id),
        path    = esc(&cf.path),
        lang    = esc(&cf.language),
        mtime   = esc(&cf.last_modified),
        imports = esc(if cf.imports.is_empty() { "[]" } else { &cf.imports }),
        hash    = esc(&cf.content_hash),
    );
    db.execute(&q)?;
    Ok(())
}

pub fn list_codefiles(db: &dyn LoomDb) -> Result<Vec<CodeFile>> {
    let q = "MATCH (cf:CodeFile) \
             RETURN cf.id, cf.path, cf.language, cf.last_modified, cf.imports, \
                    cf.content_hash \
             ORDER BY cf.path";
    let result = db.execute(q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_codefile(row, &cols)).collect())
}

/// Store the content fingerprint (see `repo::content_hash`) on a CodeFile —
/// written by `loom sync`, read back as the next sync's change baseline.
pub fn update_codefile_hash(db: &dyn LoomDb, id: &str, hash: &str) -> Result<()> {
    db.execute(&format!(
        "MATCH (cf:CodeFile {{id: '{}'}}) SET cf.{h} = '{}'",
        esc(id), esc(hash),
        h = crate::db::schema::prop::CONTENT_HASH,
    ))?;
    Ok(())
}

/// Store the statically-extracted import list (JSON array of repo-relative
/// paths) on a CodeFile — written by `loom sync`, read by smells/discovery.
pub fn update_codefile_imports(db: &dyn LoomDb, id: &str, imports_json: &str) -> Result<()> {
    db.execute(&format!(
        "MATCH (cf:CodeFile {{id: '{}'}}) SET cf.{imports} = '{}'",
        esc(id), esc(imports_json),
        imports = crate::db::schema::prop::IMPORTS,
    ))?;
    Ok(())
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

/// Resolve a CodeFile by id or by exact path (paths are unique — `codefile add`
/// skips already-registered paths).
pub fn get_codefile_by_id_or_path(db: &dyn LoomDb, key: &str) -> Result<Option<CodeFile>> {
    Ok(list_codefiles(db)?
        .into_iter()
        .find(|c| c.id == key || c.path == key))
}

/// Remove a CodeFile node and every edge attached to it (its IMPLEMENTS
/// groundings die with it). For dropping phantoms after a file is deleted or
/// renamed on disk — affected intents become unrealized leaves again, which the
/// compass routes back to `ground`. Returns the removed file, if found.
pub fn delete_codefile(db: &dyn LoomDb, key: &str) -> Result<Option<CodeFile>> {
    let Some(cf) = get_codefile_by_id_or_path(db, key)? else {
        return Ok(None);
    };
    db.execute(&format!(
        "MATCH (cf:CodeFile {{id: '{}'}}) DETACH DELETE cf",
        esc(&cf.id)
    ))?;
    Ok(Some(cf))
}

fn row_to_codefile(row: &[Value], cols: &HashMap<&str, usize>) -> CodeFile {
    CodeFile {
        id:            str_val(get(row, cols, "cf.id")),
        path:          str_val(get(row, cols, "cf.path")),
        language:      str_val(get(row, cols, "cf.language")),
        last_modified: str_val(get(row, cols, "cf.last_modified")),
        imports:       str_val(get(row, cols, "cf.imports")),
        content_hash:  str_val(get(row, cols, "cf.content_hash")),
    }
}
