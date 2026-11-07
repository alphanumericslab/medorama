-- Schema version 3 (SQLite): the privacy gate's state kept by the server.

-- a session's Privacy Mode choice ('on', 'off'; NULL: none made, the user's default): the
-- server's, never the browser's (signin.rs `set_privacy`)
ALTER TABLE sessions ADD COLUMN privacy TEXT;

-- moves whenever an identifier the gate's dictionary holds changes (gate.rs `Dict::build`):
-- patients' and encounters' IDs and pseudonyms, aliases, MRNs, account numbers, recordings'
-- paths and links.  The server rebuilds the dictionary when it moves (server.rs `restricted`).
ALTER TABLE data_version ADD COLUMN identifiers INTEGER NOT NULL DEFAULT 0;
CREATE TRIGGER IF NOT EXISTS identifiers_patients_add AFTER INSERT ON patients BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_patients_change AFTER UPDATE OF patient_id, pseudonym, date_draw ON patients BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_patients_remove AFTER DELETE ON patients BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_encounters_add AFTER INSERT ON encounters BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_encounters_change AFTER UPDATE OF encounter_id, pseudonym, patient ON encounters BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_encounters_remove AFTER DELETE ON encounters BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_aliases_add AFTER INSERT ON pseudonym_aliases BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_aliases_change AFTER UPDATE ON pseudonym_aliases BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_aliases_remove AFTER DELETE ON pseudonym_aliases BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_mrns_add AFTER INSERT ON patient_mrns BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_mrns_change AFTER UPDATE ON patient_mrns BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_mrns_remove AFTER DELETE ON patient_mrns BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_patient_ids_add AFTER INSERT ON patient_identifiers BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_patient_ids_change AFTER UPDATE OF mrn, patient ON patient_identifiers BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_patient_ids_remove AFTER DELETE ON patient_identifiers BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_encounter_ids_add AFTER INSERT ON encounter_identifiers BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_encounter_ids_change AFTER UPDATE OF csn, encounter ON encounter_identifiers BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_encounter_ids_remove AFTER DELETE ON encounter_identifiers BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_recordings_add AFTER INSERT ON recordings BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_recordings_change AFTER UPDATE OF path, name ON recordings BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_recordings_remove AFTER DELETE ON recordings BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_links_add AFTER INSERT ON recording_links BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_links_change AFTER UPDATE OF recording, encounter ON recording_links BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
CREATE TRIGGER IF NOT EXISTS identifiers_links_remove AFTER DELETE ON recording_links BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
END;
