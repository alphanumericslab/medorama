-- Medorama's control store, SQLite (docs/design/databases-only.md, "The control store").
--
-- It holds no patient data: no names, no identifiers, no values.  Patients are numbers and
-- pseudonyms (MO-…); what Medorama knows of each connected database's persons, recordings and
-- variables is in that database's folder of the workspace, encrypted with its own key
-- (workspace.rs).  The connected databases themselves are read in place and never written.
--
-- Times are seconds since 1970-01-01 UTC (REAL). JSON columns hold UTF-8 JSON text.

-- ------------------------------------------------------------------ users and sessions

CREATE TABLE users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    username      TEXT NOT NULL UNIQUE,
    display_name  TEXT NOT NULL DEFAULT '',
    role          TEXT NOT NULL CHECK (role IN ('admin', 'analyst', 'viewer')),
    phi           INTEGER NOT NULL DEFAULT 0,      -- may see identifiers (of the databases granted with phi)
    password_hash TEXT NOT NULL,                   -- Argon2id, PHC string
    disabled      INTEGER NOT NULL DEFAULT 0,
    created_at    REAL NOT NULL
);

CREATE TABLE sessions (
    token_hash  TEXT PRIMARY KEY,                  -- SHA-256 of the cookie value
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  REAL NOT NULL,
    expires_at  REAL NOT NULL,
    last_seen   REAL,                              -- the user's last request (idle sign-out)
    privacy     TEXT                               -- 'on', 'off'; NULL: the user's default (signin.rs)
);

-- ------------------------------------------------------------------ connected databases (databases.rs)

CREATE TABLE databases (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    name               TEXT NOT NULL UNIQUE,
    kind               TEXT NOT NULL CHECK (kind IN ('omop', 'views', 'dbb')),
    url                TEXT NOT NULL,              -- without a password
    password_file      TEXT NOT NULL DEFAULT '',
    password_wrapped   TEXT NOT NULL DEFAULT '',   -- a password given to Medorama, wrapped by the master key
    ca_file            TEXT NOT NULL DEFAULT '',
    settings           TEXT NOT NULL DEFAULT '{}', -- JSON (sources/mod.rs, Settings)
    status             TEXT NOT NULL DEFAULT 'unknown' CHECK (status IN ('online', 'offline', 'unknown', 'removing')),
    status_error       TEXT NOT NULL DEFAULT '',
    checked_at         REAL,
    data_version       TEXT NOT NULL DEFAULT '',   -- as the database says, when last checked
    variables_version  TEXT NOT NULL DEFAULT '',   -- the data version its variables were read at
    created_by         INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at         REAL NOT NULL
);

-- derived databases (an action's results) and the databases they come from
CREATE TABLE derived (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    action      TEXT NOT NULL,
    created_at  REAL NOT NULL
);
CREATE TABLE derived_sources (
    derived_id   INTEGER NOT NULL REFERENCES derived(id) ON DELETE CASCADE,
    database_id  INTEGER NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
    PRIMARY KEY (derived_id, database_id)
);
CREATE INDEX derived_sources_database ON derived_sources(database_id);

-- each workspace owner's data key, wrapped by the master key (workspace.rs)
CREATE TABLE keys (
    owner       TEXT PRIMARY KEY,                  -- db:ID, cohort:ID, derived:ID
    wrapped     TEXT NOT NULL,                     -- hex: nonce and the AES-256-GCM sealed key
    master_id   TEXT NOT NULL,                     -- which master key wrapped it
    created_at  REAL NOT NULL
);

-- who may use a connected database or a cohort (grants.rs)
CREATE TABLE grants (
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    object      TEXT NOT NULL,                     -- db:ID, cohort:ID
    permission  TEXT NOT NULL CHECK (permission IN ('search', 'view', 'phi', 'edit', 'manage')),
    granted_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    granted_at  REAL NOT NULL,
    PRIMARY KEY (user_id, object, permission)
);
CREATE INDEX grants_object ON grants(object);

-- ------------------------------------------------------------------ patients (persons.rs)

-- Medorama's patient numbers: no identifier, only the pseudonym Privacy Mode shows
CREATE TABLE patients (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    pseudonym   TEXT,                              -- MO-XXXX-XXXX, drawn at random when first linked
    date_draw   REAL,                              -- this patient's draw for Privacy Mode's date offset (deid.rs)
    created_at  REAL NOT NULL
);
CREATE UNIQUE INDEX patients_pseudonym ON patients(pseudonym);

-- the identifiers that link databases' persons to one patient, as keyed hashes: HMAC of
-- (issuer, identifier), so that equal identifiers join and none can be read back
CREATE TABLE patient_keys (
    hash         TEXT NOT NULL,
    database_id  INTEGER NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
    patient      INTEGER NOT NULL REFERENCES patients(id) ON DELETE CASCADE,
    PRIMARY KEY (hash, database_id)
);
CREATE INDEX patient_keys_patient ON patient_keys(patient);
CREATE INDEX patient_keys_database ON patient_keys(database_id);

-- the pseudonyms of visits (MO-E-…), drawn at random when a visit is first shown; the visit is
-- known by a keyed hash of its database's key for it
CREATE TABLE visit_pseudonyms (
    database_id  INTEGER NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
    visit_hash   TEXT NOT NULL,
    pseudonym    TEXT NOT NULL UNIQUE,
    PRIMARY KEY (database_id, visit_hash)
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

-- one action on one recording of a recording database; a done job is a result
CREATE TABLE jobs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    run             INTEGER REFERENCES runs(id) ON DELETE SET NULL,
    action          TEXT NOT NULL,
    action_version  TEXT NOT NULL,
    params          TEXT NOT NULL DEFAULT '{}',    -- JSON
    params_hash     TEXT NOT NULL,
    database_id     INTEGER NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
    recording       INTEGER NOT NULL,       -- its number: database × 1,000,000,000 + its number there
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
CREATE INDEX jobs_recording ON jobs(recording, action, id);  -- a recording's number names its database

-- patients each user has looked at
CREATE TABLE history (
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    patient     INTEGER NOT NULL REFERENCES patients(id) ON DELETE CASCADE,
    first_seen  REAL NOT NULL,
    last_seen   REAL NOT NULL,
    views       INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (user_id, patient)
);

-- ------------------------------------------------------------------ cohorts (cohorts/)

-- a cohort's definition; its members are in the workspace (cohorts/c-NNNN.sqlite)
CREATE TABLE cohorts (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    name             TEXT NOT NULL,
    description      TEXT NOT NULL DEFAULT '',
    owner            INTEGER REFERENCES users(id) ON DELETE SET NULL,   -- NULL: the local command line
    criteria         TEXT NOT NULL DEFAULT '{}',   -- JSON (docs/user-guide.md, "Cohorts")
    databases        TEXT NOT NULL DEFAULT '[]',   -- JSON: the databases it searches (ids; empty: all granted)
    source           TEXT,                          -- JSON: the cohorts it is made from (a subcohort, a merge), or NULL
    origin           TEXT NOT NULL DEFAULT '',      -- how it was made (cloned from …)
    created_at       REAL NOT NULL,
    updated_at       REAL NOT NULL,
    refreshed_at     REAL,
    source_versions  TEXT NOT NULL DEFAULT '{}'     -- JSON: each database's version when its members were found
);

-- ------------------------------------------------------------------ the audit log (audit.rs)

CREATE TABLE audit (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    time       REAL NOT NULL,
    user_id    INTEGER,                             -- kept after the user is deleted
    username   TEXT NOT NULL DEFAULT '',
    event      TEXT NOT NULL,                       -- login, connect, remove, view, ...
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

-- the settings made from the page (Privacy Mode, dates and times, ...)
CREATE TABLE settings (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,                      -- JSON
    updated_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    updated_at  REAL NOT NULL
);

-- `version` moves whenever the data change (a database connected, removed or refreshed,
-- patients linked): open pages compare it to know when to redraw (live.js).  `identifiers`
-- moves when an identifier the privacy gate's dictionary holds may have changed (gate.rs).
CREATE TABLE data_version (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    version      INTEGER NOT NULL,
    identifiers  INTEGER NOT NULL DEFAULT 0
);
INSERT INTO data_version (id, version, identifiers) VALUES (1, 0, 0);
CREATE TRIGGER data_version_database_add AFTER INSERT ON databases BEGIN
    UPDATE data_version SET version = version + 1, identifiers = identifiers + 1;
END;
CREATE TRIGGER data_version_database_change AFTER UPDATE OF status, data_version, variables_version ON databases BEGIN
    UPDATE data_version SET version = version + 1;
END;
CREATE TRIGGER data_version_database_remove AFTER DELETE ON databases BEGIN
    UPDATE data_version SET version = version + 1, identifiers = identifiers + 1;
END;
CREATE TRIGGER data_version_patient_add AFTER INSERT ON patients BEGIN
    UPDATE data_version SET version = version + 1, identifiers = identifiers + 1;
END;
CREATE TRIGGER data_version_patient_remove AFTER DELETE ON patients BEGIN
    UPDATE data_version SET version = version + 1, identifiers = identifiers + 1;
END;
