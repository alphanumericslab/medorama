//! Pseudonyms (docs/security.md, "Pseudonyms"): how Privacy Mode names patients and encounters.
//!
//! Each patient gets `MO-XXXX-XXXX` and each visit `MO-E-XXXX-XXXX` when first seen:
//! seven characters drawn from the operating system's secure random generator, then a check
//! character, in Crockford's base 32 (digits and capitals without I, L, O, U, so that nothing
//! reads as another letter).  A pseudonym is never derived from the patient's identifiers: no
//! function of an MRN gives it (HIPAA §164.514(c); ISO/TS 25237), and the only link between the
//! two is Medorama's own: the control store's patient number, and the encrypted persons file of
//! each database in the workspace (persons.rs).  It does not change while the patient is linked:
//! it goes when the last database that holds them is removed.
//!
//! The check character (Luhn mod 32) catches a mistyped character and most swapped pairs, so
//! that a pseudonym typed into a search is either right or refused.

use crate::sql::{params, Connection, OptionalExtension};

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

/// A new patient's draw for Privacy Mode's own date offset ([`assign_patient`]).
pub fn new_draw() -> anyhow::Result<f64> {
    random_draw()
}

/// A new pseudonym that no patient (or visit) has.
fn fresh(c: &Connection, visit: bool) -> anyhow::Result<String> {
    for _ in 0..100 {
        let p = format!("MO-{}{}", if visit { "E-" } else { "" }, draw()?);
        let sql = if visit {
            "SELECT EXISTS (SELECT 1 FROM visit_pseudonyms WHERE pseudonym = ?1)"
        } else {
            "SELECT EXISTS (SELECT 1 FROM patients WHERE pseudonym = ?1)"
        };
        if !c.query_row(sql, [&p], |r| r.get::<_, bool>(0))? {
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

/// The pseudonym of a database's visit (`visit_hash`: its key, hashed with the gate's secret),
/// drawn the first time it is asked for.
pub fn of_visit(c: &Connection, database: i64, visit_hash: &str) -> anyhow::Result<String> {
    let q = "SELECT pseudonym FROM visit_pseudonyms WHERE database_id = ?1 AND visit_hash = ?2";
    if let Some(p) = c.query_row(q, params![database, visit_hash], |r| r.get::<_, String>(0)).optional()? {
        return Ok(p);
    }
    let p = fresh(c, true)?;
    c.execute(
        "INSERT INTO visit_pseudonyms (database_id, visit_hash, pseudonym) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
        params![database, visit_hash, p],
    )?;
    // another request may have drawn one first: the one stored
    Ok(c.query_row(q, params![database, visit_hash], |r| r.get(0))?)
}

/// The patient a pseudonym names.
pub fn patient_of(c: &Connection, pseudonym: &str) -> anyhow::Result<Option<i64>> {
    let p = pseudonym.trim().to_ascii_uppercase();
    if !valid(&p) {
        return Ok(None);
    }
    Ok(c.query_row("SELECT id FROM patients WHERE pseudonym = ?1", [&p], |r| r.get(0)).optional()?)
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
