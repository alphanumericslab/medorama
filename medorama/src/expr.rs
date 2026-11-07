//! Cohort criteria written as text (docs/user-guide.md, "Cohorts"), e.g.
//!
//! ```text
//! (age > 18 AND age < 85 AND female) OR (age > 18 AND age < 65 AND male AND QT > 400ms)
//! ```
//!
//! parsed into the structured query of `cohorts.rs` (an `Expr`).  Words:
//!
//! | written | means |
//! |---|---|
//! | `A AND B`, `A OR B`, `NOT A`, `( … )` | AND before OR; `and`, `or`, `not` in any case, `&&`, `\|\|`, `!` too |
//! | `female`, `male` (`F`, `M`) | the patient's sex |
//! | `age > 18` (`<`, `>=`, `<=`, `=`, `!=`) | age now (PHI permission) |
//! | `has ecg`, or a category alone: `icu`, `holter`, `echo` … | has data of that category |
//! | `diagnosis "I21"`, `dx sepsis` | a condition's code starts with it, or its name contains it |
//! | `medication heparin` (`med`, `drug`) | a drug named like it |
//! | `text "pulmonary edema"` (`mentions`) | an item's text contains it |
//! | `in "ecg_index"` | the database holds data about the patient |
//! | `QT > 400ms`, `"R axis" < -30`, `ecg_index.QT > 0.4 s` | a database's column (or an EHR measurement) compared, in its unit |
//! | `latest QTc > 470` | only each patient's latest value counts (else: any value, ever) |
//! | `ecg_index.interpretation contains "fibrillation"` (`~`) | a column's text |
//!
//! A bare name (`QT`) is a database's column when exactly one database has it, else EHR items of
//! that name in one database; when several databases have it, the database must be named
//! (`ecg_index.QT`): columns of different databases are never merged.

use crate::cohorts::{Criterion, Expr, Group};

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    LParen,
    RParen,
    And,
    Or,
    Not,
    Cmp(&'static str),
    Contains,
    Num(f64),
    Str(String),
    Word(String),
}

fn tokens(s: &str) -> anyhow::Result<Vec<(usize, Tok)>> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        let at = i;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let two: String = cs[i..(i + 2).min(cs.len())].iter().collect();
        let t = match c {
            '(' => {
                i += 1;
                Tok::LParen
            }
            ')' => {
                i += 1;
                Tok::RParen
            }
            '"' | '\'' | '“' | '”' => {
                let close = |x: char| x == c || (c == '“' && x == '”') || (c == '”' && x == '”');
                let mut j = i + 1;
                while j < cs.len() && !close(cs[j]) {
                    j += 1;
                }
                if j >= cs.len() {
                    anyhow::bail!("at {}: a quote is not closed", at + 1);
                }
                let t: String = cs[i + 1..j].iter().collect();
                i = j + 1;
                Tok::Str(t)
            }
            _ if ["&&", "||", ">=", "<=", "==", "!="].contains(&two.as_str()) => {
                i += 2;
                match two.as_str() {
                    "&&" => Tok::And,
                    "||" => Tok::Or,
                    ">=" => Tok::Cmp(">="),
                    "<=" => Tok::Cmp("<="),
                    "==" => Tok::Cmp("="),
                    _ => Tok::Cmp("!="),
                }
            }
            '>' | '<' | '=' | '≥' | '≤' | '≠' | '~' | '!' => {
                i += 1;
                match c {
                    '>' => Tok::Cmp(">"),
                    '<' => Tok::Cmp("<"),
                    '=' => Tok::Cmp("="),
                    '≥' => Tok::Cmp(">="),
                    '≤' => Tok::Cmp("<="),
                    '≠' => Tok::Cmp("!="),
                    '~' => Tok::Contains,
                    _ => Tok::Not,
                }
            }
            _ if c.is_ascii_digit()
                || ((c == '-' || c == '+' || c == '.') && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit())) =>
            {
                let mut j = i + 1;
                while j < cs.len()
                    && (cs[j].is_ascii_digit()
                        || cs[j] == '.'
                        || ((cs[j] == 'e' || cs[j] == 'E')
                            && cs.get(j + 1).is_some_and(|d| d.is_ascii_digit() || *d == '-')))
                {
                    j += if cs[j] == 'e' || cs[j] == 'E' { 2 } else { 1 };
                }
                let t: String = cs[i..j].iter().collect();
                let v: f64 = t
                    .parse()
                    .ok()
                    .filter(|v: &f64| v.is_finite())
                    .ok_or_else(|| anyhow::anyhow!("at {}: not a number: {t}", at + 1))?;
                i = j;
                out.push((at, Tok::Num(v)));
                // a unit written against the number: 400ms, 1.2mV, 90%
                let mut k = i;
                while k < cs.len() && (cs[k].is_alphabetic() || cs[k] == 'µ' || cs[k] == '%' || cs[k] == '/') {
                    k += 1;
                }
                if k > i {
                    out.push((i, Tok::Word(cs[i..k].iter().collect())));
                    i = k;
                }
                continue;
            }
            _ if c.is_alphanumeric() || c == '_' || c == 'µ' || c == '°' => {
                let mut j = i;
                while j < cs.len() && (cs[j].is_alphanumeric() || "_.-µ°/%".contains(cs[j])) {
                    j += 1;
                }
                let w: String = cs[i..j].iter().collect();
                i = j;
                match w.to_lowercase().as_str() {
                    "and" => Tok::And,
                    "or" => Tok::Or,
                    "not" => Tok::Not,
                    "contains" | "mentions_text" => Tok::Contains,
                    _ => Tok::Word(w),
                }
            }
            _ => anyhow::bail!("at {}: unexpected {c:?}", at + 1),
        };
        out.push((at, t));
    }
    Ok(out)
}

/// What a name refers to: the parser asks (qualifier, name), the catalog answers with a
/// criterion of kind `column` or `measurement` naming its database.
pub type Resolve<'a> = dyn FnMut(Option<&str>, &str) -> anyhow::Result<Criterion> + 'a;

struct P<'a, 'b> {
    t: Vec<(usize, Tok)>,
    i: usize,
    /// How many `(`, `NOT` and `latest` are open: bounded so that no text can exhaust the stack.
    depth: usize,
    resolve: &'a mut Resolve<'b>,
}

/// The longest criteria text read, in bytes.
pub const MAX_LEN: usize = 8 * 1024;

/// The deepest nesting of `(`, `NOT` and `latest` read.
const MAX_DEPTH: usize = 64;

/// Units read after a number (`m` is not one: `age > 18 m` means male, `QT > 400 m` too).
const UNITS: &[&str] = &[
    "ms", "s", "sec", "min", "h", "uv", "µv", "mv", "v", "bpm", "%", "kg", "g", "mg", "cm", "mm", "°", "deg", "mmhg",
    "mmol/l", "mg/dl", "g/dl", "ng/l", "ng/ml",
];

/// The units an age may be written in (`age > 18 years`): years only.
const AGE_UNITS: &[&str] = &["y", "yr", "yrs", "year", "years"];

impl P<'_, '_> {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i).map(|(_, t)| t)
    }
    fn at(&self) -> usize {
        self.t.get(self.i).map(|(a, _)| a + 1).unwrap_or(0)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.t.get(self.i).map(|(_, t)| t.clone());
        self.i += 1;
        t
    }
    fn err<T>(&self, msg: &str) -> anyhow::Result<T> {
        match self.t.get(self.i) {
            Some((a, _)) => anyhow::bail!("at {}: {msg}", a + 1),
            None => anyhow::bail!("at the end: {msg}"),
        }
    }

    fn or(&mut self) -> anyhow::Result<Expr> {
        let mut of = vec![self.and()?];
        while self.peek() == Some(&Tok::Or) {
            self.i += 1;
            of.push(self.and()?);
        }
        Ok(if of.len() == 1 { of.pop().unwrap() } else { Expr::Group(Group { op: "or".into(), of }) })
    }

    fn and(&mut self) -> anyhow::Result<Expr> {
        let mut of = vec![self.factor()?];
        loop {
            match self.peek() {
                Some(Tok::And) => {
                    self.i += 1;
                    of.push(self.factor()?);
                }
                // two conditions side by side mean both
                Some(Tok::LParen | Tok::Not | Tok::Word(_) | Tok::Str(_)) => of.push(self.factor()?),
                _ => break,
            }
        }
        Ok(if of.len() == 1 { of.pop().unwrap() } else { Expr::Group(Group { op: "and".into(), of }) })
    }

    fn factor(&mut self) -> anyhow::Result<Expr> {
        if self.depth >= MAX_DEPTH {
            return self.err(&format!("more than {MAX_DEPTH} levels of brackets or NOT"));
        }
        self.depth += 1;
        let e = self.factor_in();
        self.depth -= 1;
        e
    }

    fn factor_in(&mut self) -> anyhow::Result<Expr> {
        match self.peek() {
            Some(Tok::Not) => {
                self.i += 1;
                let e = self.factor()?;
                Ok(match e {
                    Expr::Leaf(mut k) => {
                        k.not = !k.not;
                        Expr::Leaf(k)
                    }
                    g => Expr::Group(Group { op: "not".into(), of: vec![g] }),
                })
            }
            Some(Tok::LParen) => {
                self.i += 1;
                let e = self.or()?;
                if self.next() != Some(Tok::RParen) {
                    self.i -= 1;
                    return self.err("a ')' is missing");
                }
                Ok(e)
            }
            Some(Tok::Word(_) | Tok::Str(_)) => Ok(Expr::Leaf(self.atom()?)),
            Some(_) => self.err("a condition is expected here"),
            None => self.err("a condition is missing"),
        }
    }

    /// The text after a keyword: a quoted string, or words up to the next operator.
    fn words(&mut self, what: &str) -> anyhow::Result<String> {
        if matches!(self.peek(), Some(Tok::Cmp("=")) | Some(Tok::Contains)) {
            self.i += 1;
        }
        match self.next() {
            Some(Tok::Str(s)) => Ok(s),
            Some(Tok::Word(w)) => Ok(w),
            Some(Tok::Num(n)) => Ok(fmt(n)),
            _ => {
                self.i -= 1;
                self.err(&format!("{what}: what?"))
            }
        }
    }

    /// `cmp number [unit]`
    fn compare(&mut self, k: &mut Criterion) -> anyhow::Result<()> {
        let Some(Tok::Cmp(op)) = self.next() else {
            self.i -= 1;
            return self.err("a comparison (>, <, >=, <=, =, !=) is expected");
        };
        let Some(Tok::Num(v)) = self.next() else {
            self.i -= 1;
            return self.err("a number is expected");
        };
        k.cmp = op.to_string();
        k.value = Some(v);
        if let Some(Tok::Word(u)) = self.peek() {
            let low = u.to_lowercase();
            if UNITS.contains(&low.as_str())
                || u.contains('/')
                || (k.kind == "age" && AGE_UNITS.contains(&low.as_str()))
            {
                k.unit = u.clone();
                self.i += 1;
            }
        }
        Ok(())
    }

    fn atom(&mut self) -> anyhow::Result<Criterion> {
        let start = self.at();
        let (word, quoted) = match self.next() {
            Some(Tok::Word(w)) => (w, false),
            Some(Tok::Str(s)) => (s, true),
            _ => {
                self.i -= 1;
                return self.err("a condition is expected here");
            }
        };
        let mut k = Criterion::default();
        let low = word.to_lowercase();
        if !quoted {
            match low.as_str() {
                "female" | "females" | "women" | "woman" | "f" => {
                    k.kind = "sex".into();
                    k.text = "F".into();
                    return Ok(k);
                }
                "male" | "males" | "men" | "man" | "m" => {
                    k.kind = "sex".into();
                    k.text = "M".into();
                    return Ok(k);
                }
                "age" => {
                    k.kind = "age".into();
                    self.compare(&mut k)?;
                    if !(k.unit.is_empty() || AGE_UNITS.contains(&k.unit.to_lowercase().as_str())) {
                        anyhow::bail!("at {start}: age is in years");
                    }
                    k.unit.clear();
                    return Ok(k);
                }
                "sex" | "gender" => {
                    k.kind = "sex".into();
                    let t = self.words("sex")?;
                    k.text = t.chars().next().map(|c| c.to_ascii_uppercase().to_string()).unwrap_or_default();
                    return Ok(k);
                }
                "has" => {
                    let t = self.words("has")?;
                    let cat = crate::categories::named(&t).ok_or_else(|| {
                        anyhow::anyhow!("at {start}: no category {t:?} (ecg, icu, holter, imaging …)")
                    })?;
                    k.kind = "has".into();
                    k.category = cat.into();
                    return Ok(k);
                }
                "diagnosis" | "dx" | "condition" => {
                    k.kind = "diagnosis".into();
                    k.text = self.words("diagnosis")?;
                    return Ok(k);
                }
                "medication" | "med" | "drug" | "medications" => {
                    k.kind = "medication".into();
                    k.text = self.words("medication")?;
                    return Ok(k);
                }
                "text" | "mentions" | "notes" => {
                    k.kind = "text".into();
                    k.text = self.words("text")?;
                    return Ok(k);
                }
                "in" => {
                    let t = self.words("in")?;
                    let mut c = (self.resolve)(Some(&t), "")?;
                    c.kind = "database".into();
                    c.name.clear();
                    return Ok(c);
                }
                "latest" | "last" => {
                    if self.depth >= MAX_DEPTH {
                        return self.err(&format!("more than {MAX_DEPTH} levels of brackets or NOT"));
                    }
                    self.depth += 1;
                    let c = self.atom();
                    self.depth -= 1;
                    let mut c = c?;
                    if !matches!(c.kind.as_str(), "column" | "measurement") {
                        anyhow::bail!("at {start}: latest applies to a measurement or a column");
                    }
                    c.latest = true;
                    return Ok(c);
                }
                _ => {}
            }
        }
        // a name: qualified (db.name, db."name") or not
        let (mut qual, mut name) = (None, word.clone());
        if !quoted {
            if let Some(stem) = word.strip_suffix('.') {
                if let Some(Tok::Str(s)) = self.peek().cloned() {
                    self.i += 1;
                    qual = Some(stem.to_string());
                    name = s;
                }
            } else if let Some(dot) = word.find('.').filter(|d| *d > 0 && *d + 1 < word.len()) {
                // `Vent.` alone is a name; `ecg_index.QT` names a database
                qual = Some(word[..dot].to_string());
                name = word[dot + 1..].to_string();
            }
        }
        // a category alone: has data of it
        if qual.is_none() && !quoted && !matches!(self.peek(), Some(Tok::Cmp(_) | Tok::Contains)) {
            if let Some(cat) = crate::categories::named(&word) {
                k.kind = "has".into();
                k.category = cat.into();
                return Ok(k);
            }
            anyhow::bail!("at {start}: what is {word:?}? (a name needs a comparison: {word} > 1)");
        }
        let mut c = (self.resolve)(qual.as_deref(), &name).map_err(|e| anyhow::anyhow!("at {start}: {e}"))?;
        match self.peek() {
            Some(Tok::Contains) => {
                self.i += 1;
                c.text = self.words("contains")?;
            }
            Some(Tok::Cmp(_)) if matches!(self.t.get(self.i + 1), Some((_, Tok::Str(_)) | (_, Tok::Word(_)))) => {
                // text equal (or not): name = "x"
                let Some(Tok::Cmp(op)) = self.next() else { unreachable!() };
                if op != "=" && op != "!=" {
                    return self.err("text is compared with = or contains");
                }
                c.text = self.words("text")?;
                c.cmp = op.to_string();
            }
            _ => self.compare(&mut c)?,
        }
        Ok(c)
    }
}

fn fmt(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e12 {
        format!("{x:.0}")
    } else {
        format!("{x}")
    }
}

/// Parse criteria text; `resolve` says what names mean.
pub fn parse(text: &str, resolve: &mut Resolve) -> anyhow::Result<Expr> {
    if text.len() > MAX_LEN {
        anyhow::bail!("the expression is too long (more than {MAX_LEN} characters)");
    }
    let t = tokens(text)?;
    if t.is_empty() {
        anyhow::bail!("the expression is empty");
    }
    let mut p = P { t, i: 0, depth: 0, resolve };
    let e = p.or()?;
    if p.i < p.t.len() {
        return p.err("this is not understood (an AND or OR missing?)");
    }
    Ok(e)
}

// ------------------------------------------------------------------ units

fn unit_scale(u: &str) -> Option<(&'static str, f64)> {
    let u = u.trim().to_lowercase().replace(['µ', 'μ'], "u").replace(' ', "");
    Some(match u.as_str() {
        "s" | "sec" | "secs" | "second" | "seconds" => ("time", 1.0),
        "ms" | "msec" => ("time", 1e-3),
        "us" | "usec" => ("time", 1e-6),
        "min" | "minutes" => ("time", 60.0),
        "h" | "hr" | "hours" => ("time", 3600.0),
        "v" => ("voltage", 1.0),
        "mv" => ("voltage", 1e-3),
        "uv" => ("voltage", 1e-6),
        "bpm" | "/min" | "beats/min" => ("rate", 1.0),
        "°" | "deg" | "degrees" => ("angle", 1.0),
        "kg" => ("mass", 1.0),
        "g" => ("mass", 1e-3),
        "mg" => ("mass", 1e-6),
        "m" => ("length", 1.0),
        "cm" => ("length", 1e-2),
        "mm" => ("length", 1e-3),
        // concentrations: by mass (g/L), by amount (mol/L); one does not convert to the other
        // without the substance's molar mass
        "g/l" => ("mass concentration", 1.0),
        "g/dl" => ("mass concentration", 10.0),
        "mg/dl" => ("mass concentration", 1e-2),
        "mg/l" | "ug/ml" => ("mass concentration", 1e-3),
        "ug/l" | "ng/ml" => ("mass concentration", 1e-6),
        "ng/l" | "pg/ml" => ("mass concentration", 1e-9),
        "mol/l" => ("amount concentration", 1.0),
        "mmol/l" => ("amount concentration", 1e-3),
        "umol/l" => ("amount concentration", 1e-6),
        "nmol/l" => ("amount concentration", 1e-9),
        _ => return None,
    })
}

/// `x` in unit `from` (a criterion's) converted to unit `to` (the data's): as it is in the same
/// unit, or when the criterion says none (whether the data hold one unit is the caller's
/// question); `None` when they do not compare: different things, a unit we do not know, or
/// data without a unit against a criterion with one.
pub fn convert(x: f64, from: &str, to: &str) -> Option<f64> {
    let (f, t) = (from.trim(), to.trim());
    if f.eq_ignore_ascii_case(t) || f.is_empty() {
        return Some(x);
    }
    if t.is_empty() {
        return None;
    }
    match (unit_scale(f), unit_scale(t)) {
        (Some((df, sf)), Some((dt, st))) if df == dt => Some(x * sf / st),
        // different things, or units we do not know: only the same unit compares
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver() -> impl FnMut(Option<&str>, &str) -> anyhow::Result<Criterion> {
        |q: Option<&str>, n: &str| {
            if n.eq_ignore_ascii_case("lactate") {
                return Ok(Criterion {
                    kind: "measurement".into(),
                    name: n.into(),
                    import: Some(1),
                    ..Default::default()
                });
            }
            match q {
                Some("ecg") | None => Ok(Criterion {
                    kind: "column".into(),
                    name: if n.is_empty() { String::new() } else { format!("{n} (ms)") },
                    import: Some(7),
                    ..Default::default()
                }),
                Some(x) => anyhow::bail!("no database {x}"),
            }
        }
    }

    fn leaf(e: &Expr) -> &Criterion {
        match e {
            Expr::Leaf(k) => k,
            _ => panic!("not a leaf: {e:?}"),
        }
    }

    #[test]
    fn the_example() {
        let mut r = resolver();
        let e = parse("(age>18 AND age<85 AND female) OR (age>18 AND age<65 AND male AND QT>400ms)", &mut r).unwrap();
        let Expr::Group(g) = &e else { panic!() };
        assert_eq!(g.op, "or");
        let Expr::Group(b) = &g.of[1] else { panic!() };
        assert_eq!(b.op, "and");
        assert_eq!(b.of.len(), 4);
        let a = leaf(&b.of[1]);
        assert_eq!((a.kind.as_str(), a.cmp.as_str(), a.value), ("age", "<", Some(65.0)));
        assert_eq!(leaf(&b.of[2]).text, "M");
        let q = leaf(&b.of[3]);
        assert_eq!(
            (q.kind.as_str(), q.name.as_str(), q.import, q.cmp.as_str(), q.value, q.unit.as_str()),
            ("column", "QT (ms)", Some(7), ">", Some(400.0), "ms")
        );
    }

    #[test]
    fn words_and_errors() {
        let mut r = resolver();
        let e =
            parse("not icu and dx I21 and latest ecg.QTc >= 0.47 s and lactate > 4 && !has holter", &mut r).unwrap();
        let Expr::Group(g) = &e else { panic!() };
        let k: Vec<&Criterion> = g.of.iter().map(leaf).collect();
        assert!(k[0].not && k[0].kind == "has" && k[0].category == "icu");
        assert_eq!((k[1].kind.as_str(), k[1].text.as_str()), ("diagnosis", "I21"));
        assert!(k[2].latest && k[2].unit == "s" && k[2].import == Some(7));
        assert_eq!(k[3].kind, "measurement");
        assert!(k[4].not && k[4].category == "holter");
        let e = parse(r#"ecg."QT" contains "long" or in ecg"#, &mut r).unwrap();
        let Expr::Group(g) = &e else { panic!() };
        assert_eq!(leaf(&g.of[0]).text, "long");
        assert_eq!(leaf(&g.of[1]).kind, "database");
        for bad in [
            "(age > 18",
            "age >",
            "QT 400",
            "age > 18 OR",
            "zzz",
            "other.QT > 1",
            "\"open",
            "latest",
            "latest (",
            "not",
            "()",
            ")",
            "dx",
            "has",
        ] {
            assert!(parse(bad, &mut r).is_err(), "{bad}");
        }
        assert!(parse("(age > 18", &mut r).unwrap_err().to_string().contains("')'"));
    }

    #[test]
    fn age_in_years() {
        let mut r = resolver();
        for e in ["age > 18 years", "age > 18 y", "age > 18 yr", "age > 18 yrs", "age>18years", "age > 18 Years"] {
            let k = parse(e, &mut r).unwrap_or_else(|x| panic!("{e}: {x}"));
            let k = leaf(&k);
            assert_eq!(
                (k.kind.as_str(), k.cmp.as_str(), k.value, k.unit.as_str()),
                ("age", ">", Some(18.0), ""),
                "{e}"
            );
        }
        let e = parse("age >= 18 years and female", &mut r).unwrap();
        let Expr::Group(g) = &e else { panic!() };
        assert_eq!(leaf(&g.of[1]).text, "F");
        assert!(parse("age > 18 ms", &mut r).unwrap_err().to_string().contains("age is in years"));
    }

    #[test]
    fn unit_words() {
        let mut r = resolver();
        // `m` after a number is male, not metres
        let e = parse("age > 18 m", &mut r).unwrap();
        let Expr::Group(g) = &e else { panic!("{e:?}") };
        assert_eq!((leaf(&g.of[0]).unit.as_str(), leaf(&g.of[1]).text.as_str()), ("", "M"));
        let e = parse("QT > 400 m", &mut r).unwrap();
        let Expr::Group(g) = &e else { panic!("{e:?}") };
        assert_eq!((leaf(&g.of[0]).unit.as_str(), leaf(&g.of[1]).kind.as_str()), ("", "sex"));
        assert!(parse("QT > 1e400", &mut r).unwrap_err().to_string().contains("not a number"));
    }

    #[test]
    fn bounded_nesting() {
        let mut r = resolver();
        // text that once overflowed the stack: refused, long or deep
        for bad in ["(".repeat(100_000), "!".repeat(100_000), "latest ".repeat(20_000)] {
            assert!(parse(&bad, &mut r).unwrap_err().to_string().contains("too long"));
        }
        for bad in [
            format!("{}female{}", "(".repeat(65), ")".repeat(65)),
            format!("{}female", "!".repeat(65)),
            format!("{}QT > 1", "latest ".repeat(65)),
        ] {
            assert!(parse(&bad, &mut r).unwrap_err().to_string().contains("levels"), "{}", &bad[..10]);
        }
        let deep = format!("{}female{}", "(".repeat(60), ")".repeat(60));
        assert_eq!(leaf(&parse(&deep, &mut r).unwrap()).text, "F");
        assert!(!leaf(&parse(&format!("{}female", "!".repeat(60)), &mut r).unwrap()).not, "an even number of NOT");
    }

    #[test]
    fn units() {
        assert_eq!(convert(400.0, "ms", "s"), Some(0.4));
        assert_eq!(convert(0.15, "mV", "µV").map(|x| x.round()), Some(150.0));
        assert_eq!(convert(5.0, "", "ms"), Some(5.0));
        assert_eq!(convert(5.0, "ms", "mV"), None);
        assert_eq!(convert(5.0, "mmol/L", "mmol/l"), Some(5.0));
        // data without a unit do not compare with a criterion that has one
        assert_eq!(convert(0.4, "s", ""), None);
        assert_eq!(convert(7.0, "", ""), Some(7.0));
        // concentrations, by mass and by amount, not one into the other
        assert_eq!(convert(1.0, "g/dL", "g/L"), Some(10.0));
        assert_eq!(convert(14.0, "ng/L", "pg/mL"), Some(14.0));
        assert_eq!(convert(7.0, "mmol/L", "mg/dL"), None);
    }
}
