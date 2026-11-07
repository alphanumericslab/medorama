//! What an import still loading holds, left out of what users see (docs/administration.md,
//! "Imports and sources").  A table is committed a chunk at a time, under a batch that stays
//! `loading` until its last chunk ([`crate::imports::LOADING`]): until then, the items only it
//! holds, and the patients, encounters, bed stays and recordings only it holds, are not shown
//! by the pages, cohorts, exports, search or counts.  These are SQL fragments: nothing is
//! read twice, and they cost next to nothing when no batch is loading.

/// SQL: the ids of the rows of `kind` (`patient`, `encounter`, `bed_stay`, `recording`) that
/// only loading batches hold (none of whose provenance is an active batch's).  A row without
/// provenance (added by hand, indexed) is not one.
pub fn loading_only(kind: &str) -> String {
    let loading = crate::imports::LOADING;
    format!(
        "(SELECT pv.row_id FROM provenance pv WHERE pv.kind = '{kind}' AND pv.import_id IN {loading}
           AND NOT EXISTS (SELECT 1 FROM provenance pq WHERE pq.kind = pv.kind AND pq.row_id = pv.row_id
                             AND pq.import_id NOT IN {loading}))"
    )
}

/// SQL condition: the patient `col` (a patients id column) is shown.
pub fn patient_shown(col: &str) -> String {
    format!("{col} NOT IN {}", loading_only("patient"))
}

/// SQL condition: the encounter `col` is shown.
pub fn encounter_shown(col: &str) -> String {
    format!("{col} NOT IN {}", loading_only("encounter"))
}

/// SQL condition: the bed stay `col` is shown.
pub fn stay_shown(col: &str) -> String {
    format!("{col} NOT IN {}", loading_only("bed_stay"))
}

/// SQL condition: the recording `col` is shown.
pub fn recording_shown(col: &str) -> String {
    format!("{col} NOT IN {}", loading_only("recording"))
}

/// SQL condition: the EHR item `a` (an alias of `ehr_items`, or `""` for the table itself)
/// is shown: its first batch (`import_id`) is not loading.  An item an active batch holds
/// keeps that batch as its first.  Items before the first one a loading batch added pass on
/// their id alone (read from the index), so a scan of every item costs little more.
pub fn item_shown(a: &str) -> String {
    let loading = crate::imports::LOADING;
    let p = if a.is_empty() { String::new() } else { format!("{a}.") };
    format!(
        "({p}id < (SELECT COALESCE(MIN(li.id), 9223372036854775807) FROM ehr_items li WHERE li.import_id IN {loading})
          OR COALESCE({p}import_id, 0) NOT IN {loading})"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_name_the_loading_batches() {
        assert!(loading_only("patient").contains(crate::imports::LOADING));
        assert!(item_shown("i").starts_with("(i.id < "));
        assert!(item_shown("").starts_with("(id < "));
    }
}
