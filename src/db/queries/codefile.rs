//! CodeFile node queries.

use anyhow::{Context, Result};
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::{CodeFile, SymbolFact};

use super::row::{col_map, get, str_val};

pub fn insert_codefile(db: &dyn LoomDb, cf: &CodeFile) -> Result<()> {
    let mut p = super::row::sparams(&[
        ("id", &cf.id),
        ("path", &cf.path),
        ("lang", &cf.language),
        ("mtime", &cf.last_modified),
        ("hash", &cf.content_hash),
    ]);
    p.insert("imports".into(), super::row::list_param(&cf.imports));
    p.insert("symbols".into(), super::row::list_param(&cf.symbols));
    p.insert("symbol_facts".into(), symbol_facts_param(&cf.symbol_facts)?);
    db.execute_with_params(
        "INSERT (:CodeFile {id: $id, path: $path, language: $lang, \
         last_modified: $mtime, imports: $imports, symbols: $symbols, \
         symbol_facts: $symbol_facts, content_hash: $hash})",
        p,
    )?;
    Ok(())
}

pub fn list_codefiles(db: &dyn LoomDb) -> Result<Vec<CodeFile>> {
    let q = "MATCH (cf:CodeFile) \
             RETURN cf.id, cf.path, cf.language, cf.last_modified, cf.imports, \
                    cf.symbols, cf.symbol_facts, cf.content_hash \
             ORDER BY cf.path";
    let result = db.execute(q)?;
    let cols = col_map(&result);
    result
        .rows()
        .iter()
        .map(|row| row_to_codefile(row, &cols))
        .collect()
}

/// Store the content fingerprint (see `repo::content_hash`) on a CodeFile —
/// written by `loom sync`, read back as the next sync's change baseline.
pub fn update_codefile_hash(db: &dyn LoomDb, id: &str, hash: &str) -> Result<()> {
    db.execute(&format!(
        "MATCH (cf:CodeFile {{id: '{}'}}) SET cf.{h} = '{}'",
        esc(id),
        esc(hash),
        h = crate::db::schema::prop::CONTENT_HASH,
    ))?;
    Ok(())
}

/// Store the content fingerprint and filesystem mtime in one CodeFile write.
/// `loom sync` already owns the CodeFile row from `list_codefiles`, so there is
/// no extra existence probe on this hot path.
pub fn update_codefile_hash_and_mtime(
    db: &dyn LoomDb,
    id: &str,
    hash: &str,
    mtime: &str,
) -> Result<()> {
    db.execute(&format!(
        "MATCH (cf:CodeFile {{id: '{}'}}) SET cf.{h} = '{}', cf.last_modified = '{}'",
        esc(id),
        esc(hash),
        esc(mtime),
        h = crate::db::schema::prop::CONTENT_HASH,
    ))?;
    Ok(())
}

/// Store the statically-extracted import list (native list of repo-relative
/// paths) on a CodeFile — written by `loom sync`, read by smells/discovery.
pub fn update_codefile_imports(db: &dyn LoomDb, id: &str, imports: &[String]) -> Result<()> {
    let mut p = super::row::sparams(&[("id", id)]);
    p.insert("imports".into(), super::row::list_param(imports));
    db.execute_with_params(
        &format!(
            "MATCH (cf:CodeFile {{id: $id}}) SET cf.{imports} = $imports",
            imports = crate::db::schema::prop::IMPORTS,
        ),
        p,
    )?;
    Ok(())
}

/// Store the top-level syntax symbol list (native list of canonical labels) on
/// a CodeFile — written by `loom sync`, read by coverage diagnostics.
pub fn update_codefile_symbols(db: &dyn LoomDb, id: &str, symbols: &[String]) -> Result<()> {
    let mut p = super::row::sparams(&[("id", id)]);
    p.insert("symbols".into(), super::row::list_param(symbols));
    db.execute_with_params(
        &format!(
            "MATCH (cf:CodeFile {{id: $id}}) SET cf.{symbols} = $symbols",
            symbols = crate::db::schema::prop::SYMBOLS,
        ),
        p,
    )?;
    Ok(())
}

/// Store parsed top-level symbol facts as JSON strings in a native list. This
/// keeps the CodeFile node additive while preserving typed diagnostics in Rust.
pub fn update_codefile_symbol_facts(
    db: &dyn LoomDb,
    id: &str,
    symbol_facts: &[SymbolFact],
) -> Result<()> {
    let mut p = super::row::sparams(&[("id", id)]);
    p.insert("symbol_facts".into(), symbol_facts_param(symbol_facts)?);
    db.execute_with_params(
        &format!(
            "MATCH (cf:CodeFile {{id: $id}}) SET cf.{symbol_facts} = $symbol_facts",
            symbol_facts = crate::db::schema::prop::SYMBOL_FACTS,
        ),
        p,
    )?;
    Ok(())
}

pub fn update_codefile_mtime(db: &dyn LoomDb, id: &str, mtime: &str) -> Result<()> {
    db.execute(&format!(
        "MATCH (cf:CodeFile {{id: '{}'}}) SET cf.last_modified = '{}'",
        esc(id),
        esc(mtime)
    ))?;
    Ok(())
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
    // The IMPLEMENTS edges died with the node — prune their notes too
    // (derived edge keys embed the codefile id).
    super::note::prune_edge_notes_touching(db, &cf.id)?;
    Ok(Some(cf))
}

fn row_to_codefile(row: &[Value], cols: &HashMap<&str, usize>) -> Result<CodeFile> {
    Ok(CodeFile {
        id: str_val(get(row, cols, "cf.id")),
        path: str_val(get(row, cols, "cf.path")),
        language: str_val(get(row, cols, "cf.language")),
        last_modified: str_val(get(row, cols, "cf.last_modified")),
        imports: super::row::list_val(get(row, cols, "cf.imports")),
        symbols: super::row::list_val(get(row, cols, "cf.symbols")),
        symbol_facts: symbol_facts_val(get(row, cols, "cf.symbol_facts"))?,
        content_hash: str_val(get(row, cols, "cf.content_hash")),
    })
}

fn symbol_facts_param(symbol_facts: &[SymbolFact]) -> Result<Value> {
    let items = symbol_facts
        .iter()
        .map(|fact| serde_json::to_string(fact).context("serialize CodeFile.symbol_facts item"))
        .collect::<Result<Vec<_>>>()?;
    Ok(super::row::list_param(&items))
}

fn symbol_facts_val(v: &Value) -> Result<Vec<SymbolFact>> {
    super::row::list_val(v)
        .into_iter()
        .map(|s| {
            serde_json::from_str(&s)
                .with_context(|| format!("parse CodeFile.symbol_facts item `{s}`"))
        })
        .collect()
}
