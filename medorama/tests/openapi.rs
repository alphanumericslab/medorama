//! The API's description (`openapi.yaml`) and the server's routes (`src/server.rs`, the `api`
//! router) name the same endpoints and methods: neither may change without the other.

use std::collections::BTreeSet;

/// (path, method) of every API route the server declares.
fn routes() -> BTreeSet<(String, String)> {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/server.rs")).unwrap();
    let start = src.find("let api = Router::new()").expect("the api router");
    let end = start + src[start..].find(";\n").unwrap();
    let mut out = BTreeSet::new();
    for line in src[start..end].lines().map(str::trim).filter(|l| l.starts_with(".route(\"")) {
        let path = line[8..].split('"').next().unwrap().to_string();
        let handlers = &line[8 + path.len()..];
        for m in ["get", "post", "put", "patch", "delete"] {
            let found = handlers.contains(&format!(" {m}("))
                || handlers.contains(&format!(".{m}("))
                || handlers.contains(&format!("::{m}("));
            if found {
                out.insert((path.clone(), m.to_string()));
            }
        }
    }
    out
}

/// (path, method) of every operation `openapi.yaml` describes.
fn described() -> BTreeSet<(String, String)> {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/openapi.yaml")).unwrap();
    let mut out = BTreeSet::new();
    let mut path: Option<String> = None;
    let mut in_paths = false;
    for line in text.lines() {
        if !line.starts_with(' ') && !line.is_empty() {
            in_paths = line == "paths:";
            path = None;
            continue;
        }
        if !in_paths {
            continue;
        }
        if let Some(p) = line.strip_prefix("  /") {
            path = Some(format!("/{}", p.trim_end().trim_end_matches(':')));
        } else if let (Some(p), Some(rest)) = (&path, line.strip_prefix("    ")) {
            for m in ["get", "post", "put", "patch", "delete"] {
                if rest.starts_with(&format!("{m}:")) {
                    out.insert((p.clone(), m.to_string()));
                }
            }
        }
    }
    out
}

#[test]
fn every_route_is_described() {
    let (r, d) = (routes(), described());
    assert!(r.len() > 60, "routes found: {}", r.len());
    let missing: Vec<_> = r.difference(&d).collect();
    let extra: Vec<_> = d.difference(&r).collect();
    assert!(missing.is_empty(), "routes not in openapi.yaml: {missing:?}");
    assert!(extra.is_empty(), "openapi.yaml describes routes the server does not have: {extra:?}");
}
