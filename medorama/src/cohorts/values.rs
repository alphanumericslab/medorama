//! Testing a value against a criterion: dates, numbers in their units.

use super::*;

pub(super) fn day(s: &str, zone: chrono_tz::Tz, end: bool) -> anyhow::Result<Option<f64>> {
    let t = parse_time(s, zone).map_err(anyhow::Error::msg)?;
    if !(end && s.trim().len() <= 10) {
        return Ok(t);
    }
    // a date's end is the end of that day: the next midnight, in the zone (a day of a change
    // of clocks is 23 or 25 hours long)
    let next = chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok().and_then(|d| d.succ_opt());
    Ok(match next {
        Some(d) => parse_time(&d.format("%Y-%m-%d").to_string(), zone).map_err(anyhow::Error::msg)?,
        None => t.map(|t| t + 86400.0),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_day_ends_at_the_next_midnight() {
        let zone: chrono_tz::Tz = "Europe/Paris".parse().unwrap();
        // 2024-03-31: the clocks go forward, the day is 23 hours long
        let (a, b) = (super::day("2024-03-31", zone, false).unwrap(), super::day("2024-03-31", zone, true).unwrap());
        assert_eq!(b.unwrap() - a.unwrap(), 23.0 * 3600.0);
        let (a, b) = (super::day("2024-01-02", zone, false).unwrap(), super::day("2024-01-02", zone, true).unwrap());
        assert_eq!(b.unwrap() - a.unwrap(), 86400.0);
    }
}

/// A criterion's dates (`from`, `to`) as bounds for each patient's times.  Dates written in
/// Privacy Mode (`privacy_dates`) are dates as shown: they are read back as the gate shows them,
/// and each patient's times are moved back by their own offset before they are compared.
pub(super) struct Dates {
    pub from: Option<f64>,
    pub to: Option<f64>,
    offsets: Option<crate::search::Offsets>,
}

impl Dates {
    pub(super) fn of(c: &Connection, k: &Criterion, zone: chrono_tz::Tz) -> anyhow::Result<Dates> {
        if !k.privacy_dates || (k.from.trim().is_empty() && k.to.trim().is_empty()) {
            return Ok(Dates { from: day(&k.from, zone, false)?, to: day(&k.to, zone, true)?, offsets: None });
        }
        let o = crate::search::Offsets::load(c)?;
        let shown = |t: &str| crate::deid::shift_text(t, o.range().0 as i64);
        Ok(Dates { from: day(&shown(&k.from), zone, false)?, to: day(&shown(&k.to), zone, true)?, offsets: Some(o) })
    }

    /// Whether any bound is set.
    pub(super) fn any(&self) -> bool {
        self.from.is_some() || self.to.is_some()
    }

    /// A patient's time as the bounds read it.
    fn at(&self, p: i64, t: f64) -> f64 {
        self.offsets.as_ref().map_or(t, |o| o.shift(Some(p), t))
    }

    /// `t >= from`, for patient `p`.
    pub(super) fn after(&self, p: i64, t: f64) -> bool {
        self.from.is_none_or(|f| self.at(p, t) >= f)
    }

    /// `t < to`, for patient `p`.
    pub(super) fn before(&self, p: i64, t: f64) -> bool {
        self.to.is_none_or(|e| self.at(p, t) < e)
    }

    /// A time within the bounds, for patient `p` (none: not when there are bounds).
    pub(super) fn holds(&self, p: i64, t: Option<f64>) -> bool {
        match t {
            Some(t) => self.after(p, t) && self.before(p, t),
            None => !self.any(),
        }
    }

    /// Bounds on real times that hold every patient's (to read only those rows): the bounds
    /// themselves, or with dates as shown, widened by the least and the most offset.
    pub(super) fn wide(&self) -> (Option<f64>, Option<f64>) {
        let (lo, hi) = self.offsets.as_ref().map_or((0.0, 0.0), |o| o.range());
        (self.from.map(|f| f + lo), self.to.map(|e| e + hi))
    }

    /// Whether the bounds move with each patient (dates as shown).
    pub(super) fn per_patient(&self) -> bool {
        self.offsets.as_ref().is_some_and(|o| o.hides_dates())
    }
}

pub(super) fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e12 {
        format!("{x:.0}")
    } else {
        format!("{}", (x * 1000.0).round() / 1000.0)
    }
}

/// Whether `v` (in `unit`) has the criterion's value: `cmp value` (converted from the
/// criterion's unit) or within `min`–`max`.
pub(super) fn test_value(k: &Criterion, v: f64, unit: &str) -> anyhow::Result<bool> {
    let conv = |x: f64| {
        crate::expr::convert(x, &k.unit, unit).ok_or_else(|| {
            anyhow::anyhow!("{} is in {unit}: {} {} does not compare with it", k.name, fmt_num(x), k.unit)
        })
    };
    if let Some(x) = k.value {
        let x = conv(x)?;
        let eps = 1e-9 * x.abs().max(1.0);
        return Ok(match k.cmp.as_str() {
            ">" => v > x + eps,
            ">=" => v >= x - eps,
            "<" => v < x - eps,
            "<=" => v <= x + eps,
            "!=" => (v - x).abs() > eps,
            _ => (v - x).abs() <= eps,
        });
    }
    let lo = k.min.map(conv).transpose()?;
    let hi = k.max.map(conv).transpose()?;
    Ok(lo.is_none_or(|m| v >= m) && hi.is_none_or(|m| v <= m))
}

/// The values in `unit` that may pass [`test_value`]: (at least, at most), each a millionth wider
/// than the test, which stays the test (for reading only those from the database).  The same
/// error as the test when the threshold does not compare with the unit.
pub(super) fn value_bounds(k: &Criterion, unit: &str) -> anyhow::Result<(Option<f64>, Option<f64>)> {
    let conv = |x: f64| {
        crate::expr::convert(x, &k.unit, unit).ok_or_else(|| {
            anyhow::anyhow!("{} is in {unit}: {} {} does not compare with it", k.name, fmt_num(x), k.unit)
        })
    };
    let wide = |x: f64| 1e-6 * x.abs().max(1.0);
    if let Some(x) = k.value {
        let x = conv(x)?;
        let (eps, w) = (1e-9 * x.abs().max(1.0), wide(x));
        return Ok(match k.cmp.as_str() {
            ">" => (Some(x + eps - w), None),
            ">=" => (Some(x - eps - w), None),
            "<" => (None, Some(x - eps + w)),
            "<=" => (None, Some(x + eps + w)),
            "!=" => (None, None),
            _ => (Some(x - eps - w), Some(x + eps + w)),
        });
    }
    let lo = k.min.map(conv).transpose()?;
    let hi = k.max.map(conv).transpose()?;
    Ok((lo.map(|m| m - wide(m)), hi.map(|m| m + wide(m))))
}

pub(super) fn has_value_test(k: &Criterion) -> bool {
    k.value.is_some() || k.min.is_some() || k.max.is_some()
}

/// Whether values in `unit` compare with the criterion's value (in its unit): those that do not
/// are left out of the test, and counted ([`Skipped`]).
pub(super) fn comparable(k: &Criterion, unit: &str) -> bool {
    !has_value_test(k) || crate::expr::convert(1.0, &k.unit, unit).is_some()
}

/// A criterion's value written without a unit compares with data of one unit only: with
/// several (mg/dL and mmol/L), which one it means must be written.
pub(super) fn one_unit(k: &Criterion, units: &[String]) -> anyhow::Result<()> {
    let mut u: Vec<&str> = units.iter().map(|u| u.trim()).filter(|u| !u.is_empty()).collect();
    u.sort_by_key(|x| x.to_lowercase());
    u.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    if has_value_test(k) && k.unit.trim().is_empty() && u.len() > 1 {
        let cmp = if k.cmp.is_empty() { ">" } else { k.cmp.as_str() };
        let value = k.value.or(k.min).or(k.max).map(fmt_num).unwrap_or_default();
        anyhow::bail!(
            "{} is in {}: write the unit of the value ({} {cmp} {value} {})",
            k.name,
            u.join(" and "),
            k.name,
            u[0]
        );
    }
    Ok(())
}

/// Values a criterion could not test (their unit does not compare with its value's), by unit,
/// and how many it could: a search leaves the first out and says how many, unless none of the
/// values compares (an error, as the criterion is wrong for these data).
#[derive(Default)]
pub(super) struct Skipped {
    by_unit: std::collections::BTreeMap<String, usize>,
    compared: usize,
}

impl Skipped {
    /// Whether a value in `unit` compares (counted either way).
    pub(super) fn compares(&mut self, k: &Criterion, unit: &str) -> bool {
        if comparable(k, unit) {
            self.compared += 1;
            true
        } else {
            *self.by_unit.entry(unit.trim().to_string()).or_default() += 1;
            false
        }
    }

    /// Done: an error when no value compared; else what was left out, in words, for the
    /// search's warnings (none: nothing).
    pub(super) fn done(&self, k: &Criterion, cache: &Cache) -> anyhow::Result<()> {
        if self.by_unit.is_empty() {
            return Ok(());
        }
        let value = k.value.or(k.min).or(k.max).map(fmt_num).unwrap_or_default();
        let written = format!("{value}{}", if k.unit.is_empty() { String::new() } else { format!(" {}", k.unit) });
        let units: Vec<&str> = self.by_unit.keys().map(|u| if u.is_empty() { "no unit" } else { u.as_str() }).collect();
        if self.compared == 0 {
            anyhow::bail!("{} is in {}: {written} does not compare with it", k.name, units.join(" and "));
        }
        let parts: Vec<String> =
            self.by_unit.iter().map(|(u, n)| format!("{n} in {}", if u.is_empty() { "no unit" } else { u })).collect();
        cache.warn(Some(format!("{}: {} left out ({written} does not compare with them)", k.name, parts.join(", "))));
        Ok(())
    }
}
