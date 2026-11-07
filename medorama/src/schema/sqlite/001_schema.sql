-- Medorama's catalog database, SQLite (docs/administration.md, "The database").
--
-- Tables marked PHI hold identifiers (names, MRNs, dates of birth, hospital numbers, file
-- contents as imported): they never leave the server except to users with the PHI permission
-- outside Privacy Mode (docs/security.md). Everything else is pseudonymous: patients and
-- encounters are known by their imported IDs and their pseudonyms (MO-…).
--
-- Times are seconds since 1970-01-01 UTC (REAL). JSON columns hold UTF-8 JSON text.

-- ------------------------------------------------------------------ users and sessions

CREATE TABLE users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    username      TEXT NOT NULL UNIQUE,
    display_name  TEXT NOT NULL DEFAULT '',
    role          TEXT NOT NULL CHECK (role IN ('admin', 'analyst', 'viewer')),
    phi           INTEGER NOT NULL DEFAULT 0,      -- may see and search identifiers
    password_hash TEXT NOT NULL,                   -- Argon2id, PHC string
    disabled      INTEGER NOT NULL DEFAULT 0,
    created_at    REAL NOT NULL
);

CREATE TABLE sessions (
    token_hash  TEXT PRIMARY KEY,                  -- SHA-256 of the cookie value
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  REAL NOT NULL,
    expires_at  REAL NOT NULL,
    last_seen   REAL                               -- the user's last request (idle sign-out)
);

-- ------------------------------------------------------------------ patients and encounters

CREATE TABLE patients (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    patient_id  TEXT NOT NULL UNIQUE,              -- as imported (pseudonymous), or GEN-… when only an MRN named the patient
    sex         TEXT NOT NULL DEFAULT '',
    created_at  REAL NOT NULL,
    pseudonym   TEXT,                              -- MO-XXXX-XXXX, drawn at random when first seen
    date_draw   REAL                               -- this patient's draw for Privacy Mode's date offset (deid.rs)
);
CREATE UNIQUE INDEX patients_pseudonym ON patients(pseudonym);

CREATE TABLE patient_identifiers (                 -- PHI
    patient     INTEGER PRIMARY KEY REFERENCES patients(id) ON DELETE CASCADE,
    mrn         TEXT NOT NULL DEFAULT '',
    last_name   TEXT NOT NULL DEFAULT '',
    first_name  TEXT NOT NULL DEFAULT '',
    birth_date  TEXT NOT NULL DEFAULT ''
);
CREATE INDEX patient_identifiers_mrn ON patient_identifiers(mrn);

-- a patient's former pseudonyms (after a merge), still resolved
CREATE TABLE pseudonym_aliases (
    pseudonym  TEXT PRIMARY KEY,
    patient    INTEGER NOT NULL REFERENCES patients(id) ON DELETE CASCADE,
    since      REAL NOT NULL
);

CREATE TABLE encounters (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    encounter_id    TEXT NOT NULL UNIQUE,          -- as imported (pseudonymous)
    patient         INTEGER NOT NULL REFERENCES patients(id) ON DELETE CASCADE,
    admit_time      REAL,
    discharge_time  REAL,
    admit_unit      TEXT NOT NULL DEFAULT '',
    disposition     TEXT NOT NULL DEFAULT '',
    created_at      REAL NOT NULL,
    pseudonym       TEXT                           -- MO-E-XXXX-XXXX
);
CREATE INDEX encounters_patient ON encounters(patient);
CREATE INDEX encounters_admit ON encounters(admit_time);
CREATE UNIQUE INDEX encounters_pseudonym ON encounters(pseudonym);

CREATE TABLE encounter_identifiers (               -- PHI
    encounter  INTEGER PRIMARY KEY REFERENCES encounters(id) ON DELETE CASCADE,
    csn        TEXT NOT NULL DEFAULT ''            -- hospital encounter / account number
);

-- where the patient was during an encounter
CREATE TABLE bed_stays (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    encounter       INTEGER NOT NULL REFERENCES encounters(id) ON DELETE CASCADE,
    unit            TEXT NOT NULL DEFAULT '',
    bed             TEXT NOT NULL DEFAULT '',
    start_time      REAL,
    end_time        REAL,
    recording_file  TEXT NOT NULL DEFAULT ''       -- a recording's file name, when a row names it
);
CREATE INDEX bed_stays_encounter ON bed_stays(encounter);
CREATE INDEX bed_stays_bed ON bed_stays(unit, bed);

-- ------------------------------------------------------------------ recordings

CREATE TABLE recordings (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    path         TEXT NOT NULL UNIQUE,             -- absolute; never written, moved or deleted
    kind         TEXT NOT NULL,                    -- 'wfdb', or the kind of the adapter that reads it
    name         TEXT NOT NULL,
    size         INTEGER NOT NULL DEFAULT 0,
    fingerprint  TEXT NOT NULL,                    -- the index key (path, size, modification time)
    unit         TEXT NOT NULL DEFAULT '',         -- from a file name of the form UNIT_BED-<Unix time>
    bed          TEXT NOT NULL DEFAULT '',
    name_time    REAL,                             -- the <Unix time> of that name
    format       TEXT NOT NULL DEFAULT '',         -- as its reader names it; filled in by indexing
    t_start      REAL,
    t_end        REAL,
    category     TEXT NOT NULL DEFAULT 'icu',      -- the Patient Journey's lane (categories.rs)
    present      INTEGER NOT NULL DEFAULT 1,       -- 0: missing at the last check
    added_at     REAL NOT NULL,
    added_by     INTEGER REFERENCES users(id) ON DELETE SET NULL,
    checked_at   REAL NOT NULL
);
CREATE INDEX recordings_bed ON recordings(unit, bed);

-- a recording's encounters (a file may span two stays)
CREATE TABLE recording_links (
    recording  INTEGER NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    encounter  INTEGER NOT NULL REFERENCES encounters(id) ON DELETE CASCADE,
    method     TEXT NOT NULL,                      -- 'file', 'bed_time' or 'manual'
    linked_at  REAL NOT NULL,
    PRIMARY KEY (recording, encounter)
);
CREATE INDEX recording_links_encounter ON recording_links(encounter);

-- identifiers a recording states in its own file, to cross-check against the databases'
CREATE TABLE recording_identifiers (                -- PHI
    recording     INTEGER PRIMARY KEY REFERENCES recordings(id) ON DELETE CASCADE,
    state         TEXT NOT NULL,                    -- 'found', 'none' (the file holds none), 'error'
    full_name     TEXT NOT NULL DEFAULT '',
    last_name     TEXT NOT NULL DEFAULT '',
    first_name    TEXT NOT NULL DEFAULT '',
    patient_id    TEXT NOT NULL DEFAULT '',         -- the ID the monitor shows (often the MRN)
    encounter_id  TEXT NOT NULL DEFAULT '',
    birth_date    TEXT NOT NULL DEFAULT '',
    sex           TEXT NOT NULL DEFAULT '',
    other_values  TEXT NOT NULL DEFAULT '',         -- JSON: further distinct values (a patient change in the file)
    error         TEXT NOT NULL DEFAULT '',
    read_at       REAL NOT NULL
);

-- a recording's vital-sign series, for the patient page (derived.rs)
CREATE TABLE recording_series (
    recording  INTEGER NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    unit       TEXT NOT NULL DEFAULT '',
    t          BLOB NOT NULL,                       -- little-endian f64 times
    v          BLOB NOT NULL,                       -- and values, with each bucket's min and max
    lo         BLOB NOT NULL,
    hi         BLOB NOT NULL,
    PRIMARY KEY (recording, name)
);

-- ------------------------------------------------------------------ actions and jobs

-- one click on "Run": actions x a selection
CREATE TABLE runs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at  REAL NOT NULL,
    actions     TEXT NOT NULL,                     -- JSON list of action names
    selection   TEXT NOT NULL,                     -- JSON description of what was selected
    force       INTEGER NOT NULL DEFAULT 0
);

-- one action on one recording; a done job is a result
CREATE TABLE jobs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    run             INTEGER REFERENCES runs(id) ON DELETE SET NULL,
    action          TEXT NOT NULL,
    action_version  TEXT NOT NULL,
    params          TEXT NOT NULL DEFAULT '{}',    -- JSON
    params_hash     TEXT NOT NULL,
    recording       INTEGER NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    fingerprint     TEXT NOT NULL,                 -- of the recording when the job was queued
    state           TEXT NOT NULL CHECK (state IN ('queued', 'running', 'done', 'failed', 'cancelled')),
    priority        INTEGER NOT NULL DEFAULT 0,
    progress        REAL NOT NULL DEFAULT 0,
    worker          TEXT NOT NULL DEFAULT '',
    heartbeat       REAL,
    created_at      REAL NOT NULL,
    started_at      REAL,
    finished_at     REAL,
    error           TEXT NOT NULL DEFAULT '',
    output_dir      TEXT NOT NULL DEFAULT '',
    summary         TEXT NOT NULL DEFAULT ''       -- JSON from the action, if any
);
CREATE INDEX jobs_queue ON jobs(state, priority, id);
CREATE INDEX jobs_recording ON jobs(recording, action);

-- encounters each user has looked at
CREATE TABLE history (
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    encounter   INTEGER NOT NULL REFERENCES encounters(id) ON DELETE CASCADE,
    first_seen  REAL NOT NULL,
    last_seen   REAL NOT NULL,
    views       INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (user_id, encounter)
);

-- ------------------------------------------------------------------ imports and provenance

-- an import: one database (a table, a folder of recordings) or a connected database
CREATE TABLE imports (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL,                     -- 'table', 'recordings', 'connection'
    origin      TEXT NOT NULL DEFAULT '',          -- file names or folder
    user_id     INTEGER REFERENCES users(id) ON DELETE SET NULL,
    time        REAL NOT NULL,
    summary     TEXT NOT NULL DEFAULT '{}',        -- JSON: counts, warnings
    name        TEXT NOT NULL DEFAULT '',
    source      TEXT NOT NULL DEFAULT '',
    state       TEXT NOT NULL DEFAULT 'active',    -- 'active', 'removed'
    removed_at  REAL
);

-- which imports and sources hold each patient, encounter, bed stay and recording
CREATE TABLE provenance (
    import_id  INTEGER NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
    source     TEXT NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('recording', 'patient', 'encounter', 'bed_stay')),
    row_id     INTEGER NOT NULL,
    PRIMARY KEY (kind, row_id, import_id, source)
);
CREATE INDEX provenance_source ON provenance(source);
CREATE INDEX provenance_import ON provenance(import_id);

-- a table database's columns as imported, and the catalog column each is read as
CREATE TABLE import_columns (
    import_id  INTEGER NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
    pos        INTEGER NOT NULL,                   -- 0-based position in the file
    header     TEXT NOT NULL,
    maps_to    TEXT NOT NULL DEFAULT '',           -- the catalog's column, '' when not mapped
    PRIMARY KEY (import_id, pos)
);

-- column mappings saved for importing further tables of the same kind
CREATE TABLE mappings (
    name        TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,                     -- 'table'
    columns     TEXT NOT NULL,                     -- JSON: catalog column -> header in the file
    updated_at  REAL NOT NULL
);

-- ------------------------------------------------------------------ identity (identity.rs, linkage.rs)

-- the values each import gave a patient's or an encounter's fields: a field shows the newest
-- import's value, and removing an import brings back the one before
CREATE TABLE field_values (
    kind       TEXT NOT NULL CHECK (kind IN ('patient', 'encounter')),
    row_id     INTEGER NOT NULL,
    field      TEXT NOT NULL,                      -- patient: sex, last_name, first_name, birth_date; encounter: admit_time, discharge_time, admit_unit, disposition, csn
    value,                                         -- as the field stores it (text, or a time as a number)
    import_id  INTEGER NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
    source     TEXT NOT NULL,
    PRIMARY KEY (kind, row_id, field, import_id, source)   -- its rowid: the order values came in
);
CREATE INDEX field_values_import ON field_values(import_id);
CREATE INDEX field_values_source ON field_values(source);

-- the MRNs each import gave a patient, with the authority that issued them (an MRN is unique
-- only within its hospital or system; '' = the default authority)
CREATE TABLE patient_mrns (                         -- PHI
    patient    INTEGER NOT NULL REFERENCES patients(id) ON DELETE CASCADE,
    authority  TEXT NOT NULL DEFAULT '',
    mrn        TEXT NOT NULL,                       -- as given
    mrn_key    TEXT NOT NULL,                       -- lower-cased, trimmed: what matching compares
    import_id  INTEGER NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
    source     TEXT NOT NULL,
    PRIMARY KEY (authority, mrn_key, patient, import_id, source)
);
CREATE INDEX patient_mrns_patient ON patient_mrns(patient);
CREATE INDEX patient_mrns_import ON patient_mrns(import_id);
CREATE INDEX patient_mrns_source ON patient_mrns(source);

-- the GEN-… ID given to a patient known only by an MRN, never given again
CREATE TABLE generated_ids (                        -- PHI (the MRN)
    authority   TEXT NOT NULL DEFAULT '',
    mrn_key     TEXT NOT NULL,
    patient_id  TEXT NOT NULL UNIQUE,
    created_at  REAL NOT NULL,
    PRIMARY KEY (authority, mrn_key)
);

-- counters kept by the catalog (the next GEN-… number)
CREATE TABLE id_counters (
    name   TEXT PRIMARY KEY,
    value  INTEGER NOT NULL
);
INSERT INTO id_counters (name, value) VALUES ('generated_patient', 0);

-- rows held back for review: they contradict the catalog (the Review tab)
CREATE TABLE held_rows (                            -- PHI (the row)
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    import_id   INTEGER NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
    source      TEXT NOT NULL,
    line        INTEGER NOT NULL,
    data        TEXT NOT NULL,                      -- JSON: the row, by catalog column
    problem     TEXT NOT NULL,                      -- 'two_patients', 'mrn_of_other_patient', 'other_mrn', 'encounter_of_other_patient'
    detail      TEXT NOT NULL,                      -- what was found, with pseudonymous IDs only
    state       TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'applied', 'dismissed')),
    decided_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    decided_at  REAL,
    created_at  REAL NOT NULL
);
CREATE INDEX held_rows_state ON held_rows(state);
CREATE INDEX held_rows_import ON held_rows(import_id);

-- decisions, kept apart from the data so that they outlive re-imports
CREATE TABLE patient_merges (
    from_patient_id  TEXT PRIMARY KEY,              -- the patient ID merged away
    into_patient_id  TEXT NOT NULL,
    user_id          INTEGER REFERENCES users(id) ON DELETE SET NULL,
    merged_at        REAL NOT NULL
);

CREATE TABLE patient_pairs (
    a_patient_id  TEXT NOT NULL,                    -- a < b
    b_patient_id  TEXT NOT NULL,
    decision      TEXT NOT NULL CHECK (decision IN ('distinct')),
    user_id       INTEGER REFERENCES users(id) ON DELETE SET NULL,
    decided_at    REAL NOT NULL,
    PRIMARY KEY (a_patient_id, b_patient_id)
);

CREATE TABLE link_decisions (
    recording     INTEGER NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    encounter_id  TEXT NOT NULL,                    -- the encounter's ID, so the decision outlives a re-import
    decision      TEXT NOT NULL CHECK (decision IN ('link', 'never')),
    user_id       INTEGER REFERENCES users(id) ON DELETE SET NULL,
    decided_at    REAL NOT NULL,
    PRIMARY KEY (recording, encounter_id)
);

-- ------------------------------------------------------------------ EHR items (ehr.rs)

-- a result, a measurement, a note, a medication, a diagnosis, an attached file, ...
CREATE TABLE ehr_items (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    encounter      INTEGER REFERENCES encounters(id) ON DELETE SET NULL,
    patient        INTEGER REFERENCES patients(id) ON DELETE SET NULL,  -- NULL: waiting for a database that names its patient
    key_encounter  TEXT NOT NULL DEFAULT '',        -- the keys the row came with: encounter_id as given
    key_csn        TEXT NOT NULL DEFAULT '',        -- PHI
    key_mrn        TEXT NOT NULL DEFAULT '',        -- PHI
    key_authority  TEXT NOT NULL DEFAULT '',
    key_patient    TEXT NOT NULL DEFAULT '',        -- patient_id as given
    domain         TEXT NOT NULL,                   -- measurement, drug, observation, condition, procedure, note
    category       TEXT NOT NULL DEFAULT '',        -- the Patient Journey's lane (categories.rs)
    time           REAL,
    end_time       REAL,
    vocabulary     TEXT NOT NULL DEFAULT '',
    code           TEXT NOT NULL DEFAULT '',
    name           TEXT NOT NULL,
    value_num      REAL,
    value_text     TEXT NOT NULL DEFAULT '',        -- a value in words, a note's text (PHI in notes)
    unit           TEXT NOT NULL DEFAULT '',
    ref_low        REAL,
    ref_high       REAL,
    flag           TEXT NOT NULL DEFAULT '',
    author         TEXT NOT NULL DEFAULT '',
    service        TEXT NOT NULL DEFAULT '',
    file_path      TEXT NOT NULL DEFAULT '',        -- an attached file
    import_id      INTEGER REFERENCES imports(id) ON DELETE CASCADE,
    source         TEXT NOT NULL DEFAULT ''
);
CREATE INDEX ehr_items_encounter ON ehr_items(encounter, time);
CREATE INDEX ehr_items_patient ON ehr_items(patient, time);
CREATE INDEX ehr_items_patient_category ON ehr_items(patient, category, time);
CREATE INDEX ehr_items_import ON ehr_items(import_id);
CREATE INDEX ehr_items_source ON ehr_items(source);
CREATE INDEX ehr_items_name ON ehr_items(lower(name));
CREATE INDEX ehr_items_category ON ehr_items(category);

-- a table database's rows as imported, with what each became. A database also has a table of
-- its columns' values as numbers, dbcol_<import> (typed.rs), made with its rows.
CREATE TABLE import_rows (                          -- PHI
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    import_id     INTEGER NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
    line          INTEGER NOT NULL,                 -- in the file (the header is line 1)
    cells         TEXT NOT NULL,                    -- JSON array of strings, in the file's column order
    status        TEXT NOT NULL,                    -- identity, item, duplicate, recording, held, invalid, skipped
    key_patient   TEXT NOT NULL DEFAULT '',
    key_mrn       TEXT NOT NULL DEFAULT '',
    key_encounter TEXT NOT NULL DEFAULT '',
    item          INTEGER REFERENCES ehr_items(id) ON DELETE SET NULL,
    recording     INTEGER REFERENCES recordings(id) ON DELETE SET NULL
);
CREATE INDEX import_rows_import ON import_rows(import_id, line);
CREATE INDEX import_rows_item ON import_rows(item);

-- every import and source that holds an item (not only the first)
CREATE TABLE ehr_provenance (
    item       INTEGER NOT NULL REFERENCES ehr_items(id) ON DELETE CASCADE,
    import_id  INTEGER NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
    source     TEXT NOT NULL,
    PRIMARY KEY (item, import_id, source)
);
CREATE INDEX ehr_provenance_import ON ehr_provenance(import_id);
CREATE INDEX ehr_provenance_source ON ehr_provenance(source);

-- the items' text, searched by the patient page and cohorts (any part of a word)
CREATE VIRTUAL TABLE item_text USING fts5(body, tokenize = 'trigram');
CREATE TRIGGER item_text_insert AFTER INSERT ON ehr_items BEGIN
    INSERT INTO item_text (rowid, body) VALUES (new.id, new.name || ' ' || new.code || ' ' || new.vocabulary || ' ' ||
        new.value_text || ' ' || new.unit || ' ' || new.author || ' ' || new.service);
END;
CREATE TRIGGER item_text_delete AFTER DELETE ON ehr_items BEGIN
    DELETE FROM item_text WHERE rowid = old.id;
END;
CREATE TRIGGER item_text_update AFTER UPDATE OF name, code, vocabulary, value_text, unit, author, service ON ehr_items BEGIN
    UPDATE item_text SET body = new.name || ' ' || new.code || ' ' || new.vocabulary || ' ' || new.value_text || ' ' ||
        new.unit || ' ' || new.author || ' ' || new.service WHERE rowid = new.id;
END;

-- ------------------------------------------------------------------ cohorts (cohorts/)

CREATE TABLE cohorts (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    name             TEXT NOT NULL,
    description      TEXT NOT NULL DEFAULT '',
    owner            INTEGER REFERENCES users(id) ON DELETE SET NULL,   -- NULL: the local command line
    criteria         TEXT NOT NULL DEFAULT '{}',   -- JSON (docs/user-guide.md, "Cohorts")
    source           TEXT,                          -- JSON: the cohorts it is made from (a subcohort, a merge), or NULL
    origin           TEXT NOT NULL DEFAULT '',      -- how it was made (cloned from …)
    created_at       REAL NOT NULL,
    updated_at       REAL NOT NULL,
    refreshed_at     REAL,
    source_versions  TEXT NOT NULL DEFAULT '{}'     -- JSON: each source's version when its members were found
);

CREATE TABLE cohort_members (
    cohort    INTEGER NOT NULL REFERENCES cohorts(id) ON DELETE CASCADE,
    patient   INTEGER NOT NULL REFERENCES patients(id) ON DELETE CASCADE,
    how       TEXT NOT NULL CHECK (how IN ('criteria', 'manual')),
    added_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    added_at  REAL NOT NULL,
    PRIMARY KEY (cohort, patient)
);
CREATE INDEX cohort_members_patient ON cohort_members(patient);

-- members removed manually: they stay out when the criteria are applied again
CREATE TABLE cohort_removed (
    cohort      INTEGER NOT NULL REFERENCES cohorts(id) ON DELETE CASCADE,
    patient     INTEGER NOT NULL REFERENCES patients(id) ON DELETE CASCADE,
    removed_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    removed_at  REAL NOT NULL,
    PRIMARY KEY (cohort, patient)
);

CREATE TABLE cohort_shares (
    cohort    INTEGER NOT NULL REFERENCES cohorts(id) ON DELETE CASCADE,
    user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    can_edit  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (cohort, user_id)
);

-- ------------------------------------------------------------------ connected databases (sources/)

CREATE TABLE connections (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    name           TEXT NOT NULL UNIQUE,
    kind           TEXT NOT NULL CHECK (kind IN ('omop', 'sql')),
    url            TEXT NOT NULL,                  -- without a password
    password_file  TEXT NOT NULL DEFAULT '',
    ca_file        TEXT NOT NULL DEFAULT '',
    settings       TEXT NOT NULL DEFAULT '{}',     -- JSON (sources/mod.rs, Settings)
    import_id      INTEGER NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
    created_by     INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at     REAL NOT NULL
);

-- a connected database's persons, linked to the catalog's patients
CREATE TABLE source_persons (                       -- PHI (the source's person key may identify)
    connection  INTEGER NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    person_key  TEXT NOT NULL,
    patient     INTEGER NOT NULL REFERENCES patients(id) ON DELETE CASCADE,
    linked_at   REAL NOT NULL,
    PRIMARY KEY (connection, person_key)
);
CREATE INDEX source_persons_patient ON source_persons(patient);

-- ------------------------------------------------------------------ the audit log (audit.rs)

CREATE TABLE audit (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    time       REAL NOT NULL,
    user_id    INTEGER,                             -- kept after the user is deleted
    username   TEXT NOT NULL DEFAULT '',
    event      TEXT NOT NULL,                       -- login, import, remove, phi_search, phi_view, ...
    target     TEXT NOT NULL DEFAULT '',
    detail     TEXT NOT NULL DEFAULT '',
    sealed_in  INTEGER                              -- the seal that covers it (tamper evidence)
);
CREATE INDEX audit_time ON audit(time);
CREATE INDEX audit_sealed_in ON audit(sealed_in);

CREATE TABLE audit_seals (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    time     REAL NOT NULL,
    entries  INTEGER NOT NULL,                      -- how many entries it covers
    hash     TEXT NOT NULL                          -- hex SHA-256: the chain after its last entry
);

-- ------------------------------------------------------------------ settings and the data version

-- the catalog's settings made from the page (Privacy Mode, dates and times, ...)
CREATE TABLE settings (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,                      -- JSON
    updated_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    updated_at  REAL NOT NULL
);

-- moves whenever the data change (an import, a removal, patients or recordings added or
-- removed): open pages compare it to know when to redraw (live.js)
CREATE TABLE data_version (
    id       INTEGER PRIMARY KEY CHECK (id = 1),
    version  INTEGER NOT NULL
);
INSERT INTO data_version (id, version) VALUES (1, 0);
CREATE TRIGGER data_version_import AFTER INSERT ON imports BEGIN
    UPDATE data_version SET version = version + 1;
END;
CREATE TRIGGER data_version_import_state AFTER UPDATE OF state ON imports BEGIN
    UPDATE data_version SET version = version + 1;
END;
CREATE TRIGGER data_version_patient_add AFTER INSERT ON patients BEGIN
    UPDATE data_version SET version = version + 1;
END;
CREATE TRIGGER data_version_patient_remove AFTER DELETE ON patients BEGIN
    UPDATE data_version SET version = version + 1;
END;
CREATE TRIGGER data_version_encounter_remove AFTER DELETE ON encounters BEGIN
    UPDATE data_version SET version = version + 1;
END;
CREATE TRIGGER data_version_recording_add AFTER INSERT ON recordings BEGIN
    UPDATE data_version SET version = version + 1;
END;
CREATE TRIGGER data_version_recording_remove AFTER DELETE ON recordings BEGIN
    UPDATE data_version SET version = version + 1;
END;
