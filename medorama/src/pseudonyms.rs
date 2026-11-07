//! Pseudonyms (docs/security.md, "Pseudonyms"): how Privacy Mode names patients and encounters.
//!
//! Each patient gets `MO-XXXX-XXXX` and each encounter `MO-E-XXXX-XXXX` when first seen:
//! seven characters drawn from the operating system's secure random generator, then a check
//! character, in Crockford's base 32 (digits and capitals without I, L, O, U, so that nothing
//! reads as another letter).  A pseudonym is never derived from the patient's identifiers: no
//! function of an MRN gives it (HIPAA §164.514(c); ISO/TS 25237), and the only link between the
//! two is this catalog's database.  It does not change: a patient keeps it through re-imports,
//! and a merged patient's pseudonym stays an alias of the patient it was merged into.
//!
//! The check character (Luhn mod 32) catches a mistyped character and most swapped pairs, so
//! that a pseudonym typed into a search is either right or refused.

use crate::sql::{params, Connection, OptionalExtension};

use crate::db::now;

/// Crockford's base 32.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const RANDOM: usize = 7;

/// The check character of `body` (Luhn mod N, N = 32).
fn check(body: &[u8]) -> u8 {
    let mut factor = 2;
    let mut sum = 0;
    for &ch in body.iter().rev() {
        let Some(code) = ALPHABET.iter().position(|a| *a == ch) else { continue };
        let mut addend = factor * code;
        factor = if factor == 2 { 1 } else { 2 };
        addend = addend / 32 + addend % 32;
        sum += addend;
    }
    ALPHABET[(32 - sum % 32) % 32]
}

/// Eight characters: seven random, one check, as `XXXX-XXXX`.
fn draw() -> anyhow::Result<String> {
    let mut bytes = [0u8; RANDOM];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("the system's random generator failed: {e}"))?;
    let mut body: Vec<u8> = bytes.iter().map(|b| ALPHABET[(*b & 31) as usize]).collect();
    body.push(check(&body));
    let s = String::from_utf8(body)?;
    Ok(format!("{}-{}", &s[..4], &s[4..]))
}

/// Whether text is a well-formed pseudonym (prefix, alphabet, check character), in any case.
pub fn valid(s: &str) -> bool {
    let s = s.trim().to_ascii_uppercase();
    let Some(rest) = s.strip_prefix("MO-E-").or_else(|| s.strip_prefix("MO-")) else { return false };
    let body: Vec<u8> = rest.bytes().filter(|b| *b != b'-').collect();
    body.len() == RANDOM + 1 && body.iter().all(|b| ALPHABET.contains(b)) && check(&body[..RANDOM]) == body[RANDOM]
}

/// `n` new pseudonyms (of encounters, or of patients), none taken nor drawn twice: drawn, then
/// checked in one query (a clash, very rare, is drawn again).
pub fn fresh_many(c: &Connection, n: usize, encounter: bool) -> anyhow::Result<Vec<String>> {
    let mut out: Vec<String> = vec![];
    let mut seen = std::collections::HashSet::new();
    for _ in 0..100 {
        let need = n - out.len();
        if need == 0 {
            break;
        }
        let mut drawn = vec![];
        while drawn.len() < need {
            let p = format!("MO-{}{}", if encounter { "E-" } else { "" }, draw()?);
            if seen.insert(p.clone()) {
                drawn.push(p);
            }
        }
        let taken: std::collections::HashSet<String> = if encounter {
            c.query_in("SELECT pseudonym FROM encounters WHERE pseudonym IN ({})", &drawn, |r| r.get(0))?
        } else {
            let mut t = c.query_in("SELECT pseudonym FROM patients WHERE pseudonym IN ({})", &drawn, |r| r.get(0))?;
            t.extend(
                c.query_in("SELECT pseudonym FROM pseudonym_aliases WHERE pseudonym IN ({})", &drawn, |r| r.get(0))?,
            );
            t
        }
        .into_iter()
        .collect();
        out.extend(drawn.into_iter().filter(|p| !taken.contains(p)));
    }
    if out.len() < n {
        anyhow::bail!("no free pseudonyms after 100 draws");
    }
    Ok(out)
}

/// A new patient's draw for Privacy Mode's own date offset ([`assign_patient`]).
pub fn new_draw() -> anyhow::Result<f64> {
    random_draw()
}

/// A new patient pseudonym that no patient (nor alias) has.
fn fresh(c: &Connection, encounter: bool) -> anyhow::Result<String> {
    for _ in 0..100 {
        let p = format!("MO-{}{}", if encounter { "E-" } else { "" }, draw()?);
        let taken: bool = if encounter {
            c.query_row("SELECT EXISTS (SELECT 1 FROM encounters WHERE pseudonym = ?1)", [&p], |r| r.get(0))?
        } else {
            c.query_row(
                "SELECT EXISTS (SELECT 1 FROM patients WHERE pseudonym = ?1)
                     OR EXISTS (SELECT 1 FROM pseudonym_aliases WHERE pseudonym = ?1)",
                [&p],
                |r| r.get(0),
            )?
        };
        if !taken {
            return Ok(p);
        }
    }
    anyhow::bail!("no free pseudonym after 100 draws")
}

/// Gives a new patient its pseudonym, and its draw for Privacy Mode's date offset: random, made
/// once, so that every session shows the patient's dates moved by the same amount (deid.rs).
pub fn assign_patient(c: &Connection, patient: i64) -> anyhow::Result<()> {
    let p = fresh(c, false)?;
    c.execute("UPDATE patients SET pseudonym = ?1, date_draw = ?2 WHERE id = ?3", params![p, random_draw()?, patient])?;
    Ok(())
}

/// A number in [0, 1), from the system's random generator (53 random bits).
fn random_draw() -> anyhow::Result<f64> {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).map_err(|e| anyhow::anyhow!("the system's random generator failed: {e}"))?;
    Ok((u64::from_le_bytes(b) >> 11) as f64 / (1u64 << 53) as f64)
}

/// Gives a new encounter its pseudonym.
pub fn assign_encounter(c: &Connection, encounter: i64) -> anyhow::Result<()> {
    let p = fresh(c, true)?;
    c.execute("UPDATE encounters SET pseudonym = ?1 WHERE id = ?2", params![p, encounter])?;
    Ok(())
}

/// Before patient `from` is merged into `into`: its pseudonym becomes an alias of `into`.
pub fn alias_on_merge(c: &Connection, from: i64, into: i64) -> anyhow::Result<()> {
    let p: Option<String> =
        c.query_row("SELECT pseudonym FROM patients WHERE id = ?1", [from], |r| r.get(0)).optional()?.flatten();
    if let Some(p) = p {
        c.execute("UPDATE pseudonym_aliases SET patient = ?1 WHERE patient = ?2", [into, from])?;
        c.execute(
            "INSERT INTO pseudonym_aliases (pseudonym, patient, since) VALUES (?1, ?2, ?3)
             ON CONFLICT (pseudonym) DO UPDATE SET patient = excluded.patient, since = excluded.since",
            params![p, into, now()],
        )?;
    }
    Ok(())
}

/// The patient a pseudonym (or an alias of one) names.
pub fn patient_of(c: &Connection, pseudonym: &str) -> anyhow::Result<Option<i64>> {
    let p = pseudonym.trim().to_ascii_uppercase();
    if !valid(&p) {
        return Ok(None);
    }
    let direct: Option<i64> =
        c.query_row("SELECT id FROM patients WHERE pseudonym = ?1", [&p], |r| r.get(0)).optional()?;
    if direct.is_some() {
        return Ok(direct);
    }
    Ok(c.query_row("SELECT patient FROM pseudonym_aliases WHERE pseudonym = ?1", [&p], |r| r.get(0)).optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawn_pseudonyms_are_valid_and_checked() {
        for _ in 0..200 {
            let p = format!("MO-{}", draw().unwrap());
            assert!(valid(&p), "{p}");
            assert!(valid(&p.to_lowercase()), "any case");
            assert_eq!(p.len(), "MO-XXXX-XXXX".len());
            assert!(!p[5..].contains(['I', 'L', 'O', 'U']), "{p}: an unambiguous alphabet");
            // one character changed: refused
            let b = p.as_bytes();
            let at = 5 + (b[6] as usize % 4);
            let mut wrong = p.clone().into_bytes();
            wrong[at] = if wrong[at] == b'0' { b'1' } else { b'0' };
            assert!(!valid(std::str::from_utf8(&wrong).unwrap()), "{p} with a typo");
        }
        assert!(valid(&format!("MO-E-{}", draw().unwrap())));
        assert!(!valid("MO-1234"));
        assert!(!valid("P900004"));
    }
}
