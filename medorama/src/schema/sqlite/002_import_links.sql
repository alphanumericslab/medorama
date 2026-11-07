-- Schema version 2 (SQLite): what an import did that removing it undoes.

-- the encounter a table's row gave a recording it names (its `file`, with its `encounter_id`):
-- linked by the import (method 'file'), not by hand, and kept while the import is
CREATE TABLE import_links (
    import_id     INTEGER NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
    recording     INTEGER NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    encounter_id  TEXT NOT NULL,                    -- the encounter's ID, as the row gave it
    PRIMARY KEY (recording, encounter_id, import_id)
);
CREATE INDEX import_links_import ON import_links(import_id);

-- a generated patient ID (GEN-…) an import replaced by the patient ID its rows gave: put
-- back when that import is removed and no other names the patient by that ID
CREATE TABLE id_takeovers (
    patient       INTEGER NOT NULL REFERENCES patients(id) ON DELETE CASCADE,
    generated_id  TEXT NOT NULL,
    patient_id    TEXT NOT NULL,
    import_id     INTEGER NOT NULL REFERENCES imports(id) ON DELETE CASCADE,
    PRIMARY KEY (patient, import_id)
);
CREATE INDEX id_takeovers_import ON id_takeovers(import_id);
