use std::collections::{BTreeMap, BTreeSet};

/// The top-level directory ("cluster") of a file path — the first path segment.
/// Two files in genuinely different areas of the tree (e.g. `routes/` vs `src/`)
/// have different clusters; sub-directories under a shared root do not.
pub(super) fn dir_cluster(path: &str) -> &str {
    path.split('/').next().unwrap_or(path)
}

/// Resolve an extracted import to a registered codefile id. Imports from a Rust
/// file (`.rs`) are module syntax — resolved through Rust's module→file mapping
/// by `resolve_rust_module`, so they never leak across crates or to the standard
/// library, and a bare `use serde;` never falls back to a loose path match
/// (H-7, and the cross-crate/stdlib mis-resolution that fabricated phantom
/// layering violations). Other languages' path-style imports (`./foo`, `pkg/bar`)
/// match by longest path substring.
pub(super) fn resolve_import<'a>(
    imp: &str,
    importer: &str,
    path_to_id: &BTreeMap<&'a str, &'a str>,
) -> Option<&'a str> {
    if importer.ends_with(".rs") || imp.contains("::") {
        return resolve_rust_module(imp, importer, path_to_id);
    }
    // Longest path match wins (more specific), for path-style imports.
    let mut best: Option<(&str, usize)> = None;
    for (p, id) in path_to_id {
        if (imp.contains(*p) || p.contains(imp))
            && best.map(|(_, len)| p.len() > len).unwrap_or(true)
        {
            best = Some((*id, p.len()));
        }
    }
    best.map(|(id, _)| id)
}

/// Resolve a Rust `use` path to a registered codefile, honoring the module tree.
///
///  * `crate`/`self`/`super` stay inside the importing file's own crate and are
///    anchored to its module path, so they never resolve to a same-named file in
///    another crate.
///  * `std`/`core`/`alloc` (and any extern head that is not a unique crate
///    directory here) resolve to nothing — a stdlib path is not a repo file, and
///    for a smell that asks a human to invert a dependency an unresolved import
///    is safer than a confidently wrong one.
///  * candidate paths are constructed exactly (`{base}{frag}.rs`,
///    `{base}{frag}/mod.rs`) and looked up, never matched by an unbounded suffix
///    — so `delivery` cannot hit `commerce_delivery.rs`.
fn resolve_rust_module<'a>(
    imp: &str,
    importer: &str,
    path_to_id: &BTreeMap<&'a str, &'a str>,
) -> Option<&'a str> {
    let segs: Vec<&str> = imp
        .split("::")
        .map(str::trim)
        .take_while(|s| !s.starts_with('{') && *s != "*" && !s.is_empty())
        .collect();
    let head = *segs.first()?;
    if matches!(head, "std" | "core" | "alloc") {
        return None;
    }
    let (base, rest): (String, &[&str]) = match head {
        "crate" => (crate_src_root(importer), &segs[1..]),
        "self" => (self_dir(importer), &segs[1..]),
        "super" => {
            let k = segs.iter().take_while(|s| **s == "super").count();
            let mut b = self_dir(importer);
            for _ in 0..k {
                b = parent_dir(&b);
            }
            (b, &segs[k..])
        }
        _ => (extern_crate_root(head, path_to_id)?, &segs[1..]),
    };
    // Leading snake_case segments are modules; a CamelCase (type) or trailing
    // item ends the module path. Longest prefix first, shortening so a
    // `crate::a::b::func` still falls back to `a/b.rs`.
    let mods: Vec<&str> = rest
        .iter()
        .copied()
        .take_while(|s| {
            s.chars()
                .next()
                .is_some_and(|c| c.is_lowercase() || c == '_')
        })
        .collect();
    for take in (1..=mods.len()).rev() {
        let frag = mods[..take].join("/");
        for cand in [format!("{base}{frag}.rs"), format!("{base}{frag}/mod.rs")] {
            if let Some(id) = path_to_id.get(cand.as_str()) {
                return Some(*id);
            }
        }
    }
    None
}

/// The crate source root of a codefile path — the prefix through the `src/`
/// segment (`pulse-machine/src/webhook.rs` → `pulse-machine/src/`), or the
/// top-level directory when there is no `src/`. `crate::` paths anchor here.
fn crate_src_root(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').collect();
    if let Some(i) = segs.iter().position(|s| *s == "src") {
        format!("{}/", segs[..=i].join("/"))
    } else if segs.len() > 1 {
        format!("{}/", segs[0])
    } else {
        String::new()
    }
}

/// The directory holding a file's own submodules (`self::`): `a/b/foo.rs` →
/// `a/b/foo/`, but a `mod.rs`/`lib.rs`/`main.rs` → its containing directory.
fn self_dir(path: &str) -> String {
    let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    if matches!(file, "mod.rs" | "lib.rs" | "main.rs") {
        prefix
    } else {
        format!("{prefix}{}/", file.strip_suffix(".rs").unwrap_or(file))
    }
}

/// The parent of a trailing-slash directory (`a/b/c/` → `a/b/`, `a/` → ``).
fn parent_dir(dir: &str) -> String {
    match dir.strip_suffix('/').unwrap_or(dir).rsplit_once('/') {
        Some((p, _)) => format!("{p}/"),
        None => String::new(),
    }
}

/// The crate name carried by a source root (`pulse-machine/src/` →
/// `pulse-machine`, `crates/foo/src/` → `foo`) — the basename of the directory
/// that holds `src/`. Used to match an extern crate head against a directory.
fn crate_name_of_root(root: &str) -> &str {
    let trimmed = root.strip_suffix('/').unwrap_or(root);
    let dir = trimmed.strip_suffix("/src").unwrap_or(trimmed);
    dir.rsplit('/').next().unwrap_or(dir)
}

/// The single crate source root whose crate name matches an extern crate head
/// (dash/underscore-insensitive). None when zero or more than one crate matches
/// — an ambiguous or absent crate is never resolved. Keyed on `crate_src_root`
/// so a nested workspace (`crates/foo/src/…`) resolves by crate, not by the
/// shared `crates/` top directory.
fn extern_crate_root(head: &str, path_to_id: &BTreeMap<&str, &str>) -> Option<String> {
    let want = head.replace('_', "-");
    let roots: BTreeSet<String> = path_to_id
        .keys()
        .map(|p| crate_src_root(p))
        .filter(|root| crate_name_of_root(root).replace('_', "-") == want)
        .collect();
    if roots.len() == 1 {
        roots.into_iter().next()
    } else {
        None
    }
}

/// A same-crate `mod`-tree edge — importer and importee share a crate source
/// root and one is an ancestor/descendant module of the other (`approval.rs` ↔
/// `approval/completion.rs`). A `mod x;` declaration is structural, not an
/// invertible architectural dependency, so it never counts as a layer crossing.
pub(super) fn is_module_tree_edge(a: &str, b: &str) -> bool {
    crate_src_root(a) == crate_src_root(b)
        && (b.starts_with(&self_dir(a)) || a.starts_with(&self_dir(b)))
}
