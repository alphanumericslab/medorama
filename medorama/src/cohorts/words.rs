//! Criteria in words.

use super::*;

/// A criterion in words (for "no …" reasons and lists).
pub fn describe(k: &Criterion) -> String {
    let range = |a: Option<f64>, b: Option<f64>| match (a, b) {
        (Some(a), Some(b)) => format!(" {}–{}", fmt_num(a), fmt_num(b)),
        (Some(a), None) => format!(" ≥ {}", fmt_num(a)),
        (None, Some(b)) => format!(" ≤ {}", fmt_num(b)),
        _ => String::new(),
    };
    match k.kind.as_str() {
        "age" => match k.value {
            Some(v) => format!("age {} {}", k.cmp, fmt_num(v)),
            None => format!("age{}", range(k.min, k.max)),
        },
        "sex" => format!("sex {}", k.text),
        "has" => categories::named(&k.category).map(categories::label).unwrap_or(&k.category).to_string(),
        "encounter" => format!("encounter{}", if k.icu == Some(true) { " with ICU stay" } else { "" }),
        "diagnosis" => format!("diagnosis {}", k.text),
        "measurement" | "column" => format!(
            "{}{}{}",
            if k.latest { "latest " } else { "" },
            crate::tables::measure_column(&k.name).map(|(n, _)| n).unwrap_or(k.name.clone()),
            match (&k.value, k.text.trim()) {
                (Some(v), _) => format!(
                    " {} {}{}",
                    k.cmp,
                    fmt_num(*v),
                    if k.unit.is_empty() { String::new() } else { format!(" {}", k.unit) }
                ),
                (None, t) if !t.is_empty() => format!(" {} “{t}”", if k.cmp.is_empty() { "contains" } else { &k.cmp }),
                _ => range(k.min, k.max),
            }
        ),
        "expression" => k.text.trim().to_string(),
        "medication" => format!("medication {}", k.text),
        "text" => format!("“{}”", k.text),
        "database" => "data in that database".into(),
        x => x.into(),
    }
}

/// An expression in words, with its groups in brackets.
pub fn describe_expr(e: &Expr) -> String {
    match e {
        Expr::Leaf(k) if k.not => format!("not {}", describe(k)),
        Expr::Leaf(k) => describe(k),
        Expr::Group(g) => {
            let parts: Vec<String> =
                g.of.iter()
                    .map(|x| match x {
                        Expr::Group(h) if h.of.len() > 1 && h.op != g.op => format!("({})", describe_expr(x)),
                        _ => describe_expr(x),
                    })
                    .collect();
            match g.op.as_str() {
                "not" => format!("not ({})", parts.join(", ")),
                op => parts.join(&format!(" {} ", op.to_uppercase())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn criteria_round_trip() {
        // groups and exclusions
        let q: Criteria = serde_json::from_str(
            r#"{"version":1,"include":{"op":"or","of":[{"kind":"sex","text":"F"},{"op":"and","of":[{"kind":"age","cmp":">","value":18},{"kind":"column","import":3,"name":"QT (ms)","cmp":">","value":400,"unit":"ms","latest":true}]}]},"exclude":[{"kind":"diagnosis","text":"I21"}]}"#,
        )
        .unwrap();
        let Some(Expr::Group(g)) = &q.include else { panic!("{q:?}") };
        assert!(matches!(&g.of[0], Expr::Leaf(k) if k.kind == "sex"));
        assert_eq!(describe_expr(q.include.as_ref().unwrap()), "sex F OR (age > 18 AND latest QT > 400 ms)");
        assert_eq!(criteria_words(&q)["exclude"][0], "diagnosis I21");
        let back: Criteria = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        assert_eq!(back, q);
    }
}
