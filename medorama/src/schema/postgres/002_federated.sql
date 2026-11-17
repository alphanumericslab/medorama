-- 0.3.0 (docs/design/connected-databases.md): Medorama keeps of each database its subjects and
-- their encounters, the names and units of its variables, and the recordings it has opened.
-- `recordings` moves when recordings are first kept (the privacy gate's dictionary takes the
-- new ones without being built again).  Each database's catalog and subjects are read again at
-- its next check: their workspace tables are new.
ALTER TABLE data_version ADD COLUMN recordings BIGINT NOT NULL DEFAULT 0;
UPDATE databases SET variables_version = '';
