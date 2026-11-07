//! SQLite's dialect, as Medorama writes it, in PostgreSQL's.
//!
//! | SQLite | PostgreSQL |
//! |---|---|
//! | `?1`, `?` | `$1`, `$n` |
//! | `INSERT OR IGNORE …` | `INSERT … ON CONFLICT DO NOTHING` |
//! | `LIKE` (ignores case) | `ILIKE` |
//! | `x IS ?1`, `x IS NOT ?1` | `x IS NOT DISTINCT FROM $1`, `x IS DISTINCT FROM $1` |
//! | `CAST(x AS REAL)`, `AS INTEGER` | `AS DOUBLE PRECISION`, `AS BIGINT` |
//! | `group_concat(x)`, `group_concat(DISTINCT x)`, `group_concat(x, sep)` | `string_agg(CAST(x AS TEXT), …)` |
//! | `max(a, b)`, `min(a, b)` | `GREATEST(a, b)`, `LEAST(a, b)` |
//! | `ORDER BY a, b DESC` (NULLs first ascending) | `ORDER BY a NULLS FIRST, b DESC NULLS LAST` |
//! | `FROM (SELECT …)` without a name | `FROM (SELECT …) AS sub1` (PostgreSQL before 16 requires one) |
//!
//! Everything else is passed as written: the SQL is kept to what both understand.  Strings
//! (`'…'`, `E'…'`, `$$…$$`, `$tag$…$tag$`), quoted names and comments (`--`, `/* … */`) are
//! copied untouched.  A bare `?` is numbered after the highest `?N` of its statement.
//! `max(a, b)` and `min(a, b)` differ on NULL: SQLite's is NULL when an argument is, `GREATEST`
//! and `LEAST` ignore NULLs; Medorama uses them on values that are never NULL.
//!
//! SQL that Medorama does not write (a connected database's views) is never translated
//! ([`super::Connection::prepare_native`]).

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Param(Option<u32>),
    Punct(char),
    /// Strings, quoted names, numbers, comments, spaces: copied.
    Other(String),
    Space,
}

fn lex(sql: &str) -> Vec<Tok> {
    let s: Vec<char> = sql.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        // E'…': a string with backslash escapes
        let escaped = (c == 'e' || c == 'E')
            && s.get(i + 1) == Some(&'\'')
            && !(i > 0 && (s[i - 1].is_alphanumeric() || s[i - 1] == '_'));
        if c == '\'' || c == '"' || escaped {
            let q = if escaped { '\'' } else { c };
            let mut j = if escaped { i + 2 } else { i + 1 };
            while j < s.len() {
                if escaped && s[j] == '\\' {
                    j += 2;
                    continue;
                }
                if s[j] == q {
                    if s.get(j + 1) == Some(&q) {
                        j += 2;
                        continue;
                    }
                    break;
                }
                j += 1;
            }
            let end = (j + 1).min(s.len());
            out.push(Tok::Other(s[i..end].iter().collect()));
            i = end;
        } else if let Some(tag) = dollar_tag(&s, i) {
            // $$…$$ or $tag$…$tag$
            let body = i + tag.len();
            let end = (body..s.len()).find(|&j| s[j..].starts_with(&tag[..])).map_or(s.len(), |j| j + tag.len());
            out.push(Tok::Other(s[i..end].iter().collect()));
            i = end;
        } else if c == '/' && s.get(i + 1) == Some(&'*') {
            // a block comment (PostgreSQL's nest)
            let (mut j, mut depth) = (i + 2, 1);
            while j < s.len() && depth > 0 {
                if s[j] == '/' && s.get(j + 1) == Some(&'*') {
                    depth += 1;
                    j += 2;
                } else if s[j] == '*' && s.get(j + 1) == Some(&'/') {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            out.push(Tok::Other(s[i..j.min(s.len())].iter().collect()));
            i = j.min(s.len());
        } else if c == '-' && s.get(i + 1) == Some(&'-') {
            let j = s[i..].iter().position(|&x| x == '\n').map_or(s.len(), |p| i + p);
            out.push(Tok::Space);
            i = j;
        } else if c.is_whitespace() {
            while i < s.len() && s[i].is_whitespace() {
                i += 1;
            }
            out.push(Tok::Space);
        } else if c == '?' {
            let mut j = i + 1;
            while j < s.len() && s[j].is_ascii_digit() {
                j += 1;
            }
            let n: String = s[i + 1..j].iter().collect();
            out.push(Tok::Param(n.parse().ok()));
            i = j;
        } else if c.is_alphabetic() || c == '_' {
            let mut j = i;
            while j < s.len() && (s[j].is_alphanumeric() || s[j] == '_') {
                j += 1;
            }
            out.push(Tok::Word(s[i..j].iter().collect()));
            i = j;
        } else if c.is_ascii_digit() {
            let mut j = i;
            while j < s.len() && (s[j].is_ascii_alphanumeric() || s[j] == '.') {
                j += 1;
            }
            out.push(Tok::Other(s[i..j].iter().collect()));
            i = j;
        } else {
            out.push(Tok::Punct(c));
            i += 1;
        }
    }
    out
}

/// The tag of a dollar-quoted string starting at `i` (`$$`, `$name$`), if one does.
fn dollar_tag(s: &[char], i: usize) -> Option<Vec<char>> {
    if s[i] != '$' || (i > 0 && (s[i - 1].is_alphanumeric() || s[i - 1] == '_')) {
        return None;
    }
    let mut j = i + 1;
    while j < s.len() && (s[j].is_alphanumeric() || s[j] == '_') {
        j += 1;
    }
    let tag = &s[i + 1..j];
    (s.get(j) == Some(&'$') && !tag.first().is_some_and(|c| c.is_ascii_digit())).then(|| s[i..=j].to_vec())
}

fn is_word(t: Option<&Tok>, w: &str) -> bool {
    matches!(t, Some(Tok::Word(x)) if x.eq_ignore_ascii_case(w))
}

/// The index of the next token that is not a space, from `i`.
fn next(t: &[Tok], i: usize) -> usize {
    let mut j = i;
    while j < t.len() && t[j] == Tok::Space {
        j += 1;
    }
    j
}

/// From the `(` at `open`: the index of its `)` and the arguments' ranges (top-level commas).
fn call(t: &[Tok], open: usize) -> Option<(usize, Vec<(usize, usize)>)> {
    let mut depth = 0;
    let mut args = vec![];
    let mut start = open + 1;
    for (j, tok) in t.iter().enumerate().skip(open) {
        match tok {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') => {
                depth -= 1;
                if depth == 0 {
                    args.push((start, j));
                    return Some((j, args));
                }
            }
            Tok::Punct(',') if depth == 1 => {
                args.push((start, j));
                start = j + 1;
            }
            _ => {}
        }
    }
    None
}

struct State {
    params: u32,
    subqueries: u32,
}

/// Words after a subquery that are not its name.
const NOT_ALIAS: &[&str] = &[
    "WHERE",
    "GROUP",
    "ORDER",
    "LIMIT",
    "OFFSET",
    "UNION",
    "EXCEPT",
    "INTERSECT",
    "JOIN",
    "LEFT",
    "RIGHT",
    "INNER",
    "CROSS",
    "FULL",
    "ON",
    "HAVING",
    "WINDOW",
    "RETURNING",
    "NATURAL",
];

/// PostgreSQL's form of some SQL, which may hold several statements.
pub fn translate(sql: &str) -> String {
    let t = lex(sql);
    let mut st = State { params: 0, subqueries: 0 };
    let mut out = String::new();
    let mut start = 0;
    for (i, tok) in t.iter().enumerate().chain(std::iter::once((t.len(), &Tok::Punct(';')))) {
        if *tok == Tok::Punct(';') {
            out.push_str(&statement(&t[start..i.min(t.len())], &mut st));
            if i < t.len() {
                out.push(';');
            }
            start = i + 1;
        }
    }
    out
}

fn statement(t: &[Tok], st: &mut State) -> String {
    // a bare `?` is the parameter after the highest numbered one
    st.params = t.iter().filter_map(|x| if let Tok::Param(Some(n)) = x { Some(*n) } else { None }).max().unwrap_or(0);
    let first = next(t, 0);
    let ignore = is_word(t.get(first), "INSERT") && is_word(t.get(next(t, first + 1)), "OR") && {
        let k = next(t, next(t, first + 1) + 1);
        is_word(t.get(k), "IGNORE")
    };
    if !ignore {
        return emit(t, st);
    }
    let k = next(t, next(t, first + 1) + 1);
    let rest = &t[k + 1..];
    // ON CONFLICT goes before RETURNING
    let mut depth = 0;
    let mut ret = rest.len();
    for (j, tok) in rest.iter().enumerate() {
        match tok {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') => depth -= 1,
            Tok::Word(w) if depth == 0 && w.eq_ignore_ascii_case("RETURNING") => {
                ret = j;
                break;
            }
            _ => {}
        }
    }
    let body = emit(&rest[..ret], st);
    let tail = emit(&rest[ret..], st);
    format!(
        "{}INSERT{} ON CONFLICT DO NOTHING{}{}",
        emit(&t[..first], st),
        body.trim_end(),
        if tail.is_empty() { "" } else { " " },
        tail
    )
}

fn emit(t: &[Tok], st: &mut State) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < t.len() {
        match &t[i] {
            Tok::Space => out.push(' '),
            Tok::Other(s) => out.push_str(s),
            Tok::Punct(c) => out.push(*c),
            Tok::Param(n) => {
                let n = n.unwrap_or_else(|| {
                    st.params += 1;
                    st.params
                });
                out.push_str(&format!("${n}"));
            }
            Tok::Word(w) => {
                let up = w.to_ascii_uppercase();
                let j = next(t, i + 1);
                match up.as_str() {
                    "LIKE" => out.push_str("ILIKE"),
                    "IS" if matches!(t.get(j), Some(Tok::Param(_))) => out.push_str("IS NOT DISTINCT FROM"),
                    "IS" if is_word(t.get(j), "NOT") && matches!(t.get(next(t, j + 1)), Some(Tok::Param(_))) => {
                        out.push_str("IS DISTINCT FROM");
                        i = j + 1;
                        continue;
                    }
                    "AS" if is_word(t.get(j), "REAL") => {
                        out.push_str("AS DOUBLE PRECISION");
                        i = j + 1;
                        continue;
                    }
                    "AS" if is_word(t.get(j), "INTEGER") => {
                        out.push_str("AS BIGINT");
                        i = j + 1;
                        continue;
                    }
                    "GROUP_CONCAT" | "MAX" | "MIN" if t.get(j) == Some(&Tok::Punct('(')) => {
                        if let Some((close, args)) = call(t, j) {
                            if up == "GROUP_CONCAT" {
                                let (a, b) = args[0];
                                let k = next(t, a);
                                let (distinct, a) =
                                    if is_word(t.get(k), "DISTINCT") { ("DISTINCT ", k + 1) } else { ("", a) };
                                let x = emit(&t[a..b], st);
                                let sep = match args.get(1) {
                                    Some(&(a, b)) => emit(&t[a..b], st).trim().to_string(),
                                    None => "','".into(),
                                };
                                out.push_str(&format!("string_agg({distinct}CAST({} AS TEXT), {sep})", x.trim()));
                                i = close + 1;
                                continue;
                            }
                            if args.len() >= 2 {
                                let parts: Vec<String> =
                                    args.iter().map(|&(a, b)| emit(&t[a..b], st).trim().to_string()).collect();
                                let f = if up == "MAX" { "GREATEST" } else { "LEAST" };
                                out.push_str(&format!("{f}({})", parts.join(", ")));
                                i = close + 1;
                                continue;
                            }
                        }
                        out.push_str(w);
                    }
                    "FROM" | "JOIN" if t.get(j) == Some(&Tok::Punct('(')) => {
                        let Some((close, _)) = call(t, j) else {
                            out.push_str(w);
                            i += 1;
                            continue;
                        };
                        let first = next(t, j + 1);
                        let query = ["SELECT", "VALUES", "WITH"].iter().any(|k| is_word(t.get(first), k));
                        out.push_str(&format!("{w} ({})", emit(&t[j + 1..close], st)));
                        let after = t.get(next(t, close + 1));
                        let named = match after {
                            Some(Tok::Word(a)) => !NOT_ALIAS.contains(&a.to_ascii_uppercase().as_str()),
                            Some(Tok::Other(o)) => o.starts_with('"'),
                            _ => false,
                        };
                        if query && !named {
                            st.subqueries += 1;
                            out.push_str(&format!(" AS sub{}", st.subqueries));
                        }
                        i = close + 1;
                        continue;
                    }
                    "ORDER" if is_word(t.get(j), "BY") => {
                        out.push_str("ORDER BY ");
                        let start = j + 1;
                        let (end, keys) = order_keys(t, start);
                        let parts: Vec<String> = keys
                            .iter()
                            .map(|&(a, b)| {
                                let k = emit(&t[a..b], st).trim().to_string();
                                let words: Vec<String> = t[a..b]
                                    .iter()
                                    .filter_map(|x| match x {
                                        Tok::Word(w) => Some(w.to_ascii_uppercase()),
                                        _ => None,
                                    })
                                    .collect();
                                if words.iter().any(|w| w == "NULLS") {
                                    k
                                } else if words.last().is_some_and(|w| w == "DESC") {
                                    format!("{k} NULLS LAST")
                                } else {
                                    format!("{k} NULLS FIRST")
                                }
                            })
                            .collect();
                        out.push_str(&parts.join(", "));
                        if end < t.len() {
                            out.push(' ');
                        }
                        i = end;
                        continue;
                    }
                    _ => out.push_str(w),
                }
            }
        }
        i += 1;
    }
    out
}

/// An ORDER BY clause from `start`: where it ends, and each key's range.
fn order_keys(t: &[Tok], start: usize) -> (usize, Vec<(usize, usize)>) {
    let mut depth = 0;
    let mut keys = vec![];
    let mut a = start;
    let mut j = start;
    while j < t.len() {
        match &t[j] {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') if depth == 0 => break,
            Tok::Punct(')') => depth -= 1,
            Tok::Punct(',') if depth == 0 => {
                keys.push((a, j));
                a = j + 1;
            }
            Tok::Word(w)
                if depth == 0
                    && ["LIMIT", "OFFSET", "UNION", "EXCEPT", "INTERSECT", "RETURNING"]
                        .contains(&w.to_ascii_uppercase().as_str()) =>
            {
                break
            }
            _ => {}
        }
        j += 1;
    }
    keys.push((a, j));
    (j, keys)
}

#[cfg(test)]
mod tests {
    use super::translate;

    #[test]
    fn statements_in_postgres() {
        assert_eq!(translate("SELECT a FROM t WHERE id = ?1 AND b = ?"), "SELECT a FROM t WHERE id = $1 AND b = $2");
        assert_eq!(translate("SELECT ? , ?"), "SELECT $1 , $2");
        assert_eq!(
            translate("INSERT OR IGNORE INTO t (a) VALUES (?1)"),
            "INSERT INTO t (a) VALUES ($1) ON CONFLICT DO NOTHING"
        );
        assert_eq!(
            translate("INSERT OR IGNORE INTO t (a) VALUES (?1) RETURNING id"),
            "INSERT INTO t (a) VALUES ($1) ON CONFLICT DO NOTHING RETURNING id"
        );
        assert_eq!(translate("WHERE name LIKE ?2 ESCAPE '\\'"), "WHERE name ILIKE $2 ESCAPE '\\'");
        assert_eq!(
            translate("WHERE t IS ?3 AND u IS NOT ?4 AND v IS NULL"),
            "WHERE t IS NOT DISTINCT FROM $3 AND u IS DISTINCT FROM $4 AND v IS NULL"
        );
        assert_eq!(
            translate("CAST(?4 AS REAL), CAST(x AS INTEGER)"),
            "CAST($4 AS DOUBLE PRECISION), CAST(x AS BIGINT)"
        );
        assert_eq!(
            translate("group_concat(DISTINCT e.category)"),
            "string_agg(DISTINCT CAST(e.category AS TEXT), ',')"
        );
        assert_eq!(translate("group_concat(unit, '|')"), "string_agg(CAST(unit AS TEXT), '|')");
        assert_eq!(translate("SELECT max(a, b), MAX(c)"), "SELECT GREATEST(a, b), MAX(c)");
        assert_eq!(
            translate("SELECT a FROM t ORDER BY time DESC, id LIMIT 5"),
            "SELECT a FROM t ORDER BY time DESC NULLS LAST, id NULLS FIRST LIMIT 5"
        );
        assert_eq!(
            translate("SELECT (SELECT x FROM u ORDER BY t LIMIT 1) FROM v ORDER BY lower(n)"),
            "SELECT (SELECT x FROM u ORDER BY t NULLS FIRST LIMIT 1) FROM v ORDER BY lower(n) NULLS FIRST"
        );
        // strings, quoted names and comments are left alone
        assert_eq!(translate("SELECT E'it\\'s ? LIKE', x FROM t"), "SELECT E'it\\'s ? LIKE', x FROM t");
        assert_eq!(
            translate("SELECT $$a ? LIKE b; c$$, $q$ LIKE $q$ FROM t"),
            "SELECT $$a ? LIKE b; c$$, $q$ LIKE $q$ FROM t"
        );
        assert_eq!(translate("SELECT /* a ? LIKE b; c */ x LIKE ?1"), "SELECT /* a ? LIKE b; c */ x ILIKE $1");
        assert_eq!(translate("SELECT a FROM t WHERE x = $1"), "SELECT a FROM t WHERE x = $1");
        assert_eq!(translate("SELECT 'a ? LIKE b; c', \"order\" FROM t"), "SELECT 'a ? LIKE b; c', \"order\" FROM t");
        assert_eq!(translate("A; B LIKE c;"), "A; B ILIKE c;");
        // subqueries get the name PostgreSQL 14 and 15 require
        assert_eq!(
            translate("SELECT MAX(t) FROM (SELECT 1 AS t UNION ALL SELECT 2)"),
            "SELECT MAX(t) FROM (SELECT 1 AS t UNION ALL SELECT 2) AS sub1"
        );
        assert_eq!(
            translate("SELECT x FROM (SELECT 1 AS x) y WHERE x = 1"),
            "SELECT x FROM (SELECT 1 AS x) y WHERE x = 1"
        );
        assert_eq!(
            translate("SELECT a FROM (SELECT a FROM t) WHERE a IN (SELECT b FROM (SELECT b FROM u))"),
            "SELECT a FROM (SELECT a FROM t) AS sub1 WHERE a IN (SELECT b FROM (SELECT b FROM u) AS sub2)"
        );
    }
}
