-- Schema version 3 (PostgreSQL): the privacy gate's state kept by the server.

-- a session's Privacy Mode choice ('on', 'off'; NULL: none made, the user's default): the
-- server's, never the browser's (signin.rs `set_privacy`)
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS privacy TEXT;

-- moves whenever an identifier the gate's dictionary holds changes (gate.rs `Dict::build`):
-- patients' and encounters' IDs and pseudonyms, aliases, MRNs, account numbers, recordings'
-- paths and links.  The server rebuilds the dictionary when it moves (server.rs `restricted`).
ALTER TABLE data_version ADD COLUMN IF NOT EXISTS identifiers BIGINT NOT NULL DEFAULT 0;
CREATE OR REPLACE FUNCTION medorama_identifier_version() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE data_version SET identifiers = identifiers + 1;
    RETURN NULL;
END $$;
DROP TRIGGER IF EXISTS identifiers_patients ON patients;
CREATE TRIGGER identifiers_patients AFTER INSERT OR UPDATE OF patient_id, pseudonym, date_draw OR DELETE ON patients FOR EACH STATEMENT EXECUTE FUNCTION medorama_identifier_version();
DROP TRIGGER IF EXISTS identifiers_encounters ON encounters;
CREATE TRIGGER identifiers_encounters AFTER INSERT OR UPDATE OF encounter_id, pseudonym, patient OR DELETE ON encounters FOR EACH STATEMENT EXECUTE FUNCTION medorama_identifier_version();
DROP TRIGGER IF EXISTS identifiers_pseudonym_aliases ON pseudonym_aliases;
CREATE TRIGGER identifiers_pseudonym_aliases AFTER INSERT OR UPDATE OR DELETE ON pseudonym_aliases FOR EACH STATEMENT EXECUTE FUNCTION medorama_identifier_version();
DROP TRIGGER IF EXISTS identifiers_patient_mrns ON patient_mrns;
CREATE TRIGGER identifiers_patient_mrns AFTER INSERT OR UPDATE OR DELETE ON patient_mrns FOR EACH STATEMENT EXECUTE FUNCTION medorama_identifier_version();
DROP TRIGGER IF EXISTS identifiers_patient_identifiers ON patient_identifiers;
CREATE TRIGGER identifiers_patient_identifiers AFTER INSERT OR UPDATE OF mrn, patient OR DELETE ON patient_identifiers FOR EACH STATEMENT EXECUTE FUNCTION medorama_identifier_version();
DROP TRIGGER IF EXISTS identifiers_encounter_identifiers ON encounter_identifiers;
CREATE TRIGGER identifiers_encounter_identifiers AFTER INSERT OR UPDATE OF csn, encounter OR DELETE ON encounter_identifiers FOR EACH STATEMENT EXECUTE FUNCTION medorama_identifier_version();
DROP TRIGGER IF EXISTS identifiers_recordings ON recordings;
CREATE TRIGGER identifiers_recordings AFTER INSERT OR UPDATE OF path, name OR DELETE ON recordings FOR EACH STATEMENT EXECUTE FUNCTION medorama_identifier_version();
DROP TRIGGER IF EXISTS identifiers_recording_links ON recording_links;
CREATE TRIGGER identifiers_recording_links AFTER INSERT OR UPDATE OF recording, encounter OR DELETE ON recording_links FOR EACH STATEMENT EXECUTE FUNCTION medorama_identifier_version();
