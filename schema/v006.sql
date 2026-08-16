-- AIDB schema v006
-- Model catalog may store a key *name*. Never the secret.
-- Applied when schema_version is 5. Do not rewrite v001.sql.

ALTER TABLE models ADD COLUMN key_name TEXT;

CREATE TRIGGER IF NOT EXISTS models_key_name_not_secret_ins
BEFORE INSERT ON models
WHEN new.key_name IS NOT NULL AND length(trim(new.key_name)) > 0
BEGIN
  SELECT CASE
    WHEN instr(new.key_name, '=') > 0
      OR instr(new.key_name, ' ') > 0
      OR length(new.key_name) > 64
      OR lower(new.key_name) LIKE 'sk-%'
    THEN RAISE(ABORT, 'models.key_name is a name, never the secret')
  END;
END;

CREATE TRIGGER IF NOT EXISTS models_key_name_not_secret_upd
BEFORE UPDATE OF key_name ON models
WHEN new.key_name IS NOT NULL AND length(trim(new.key_name)) > 0
BEGIN
  SELECT CASE
    WHEN instr(new.key_name, '=') > 0
      OR instr(new.key_name, ' ') > 0
      OR length(new.key_name) > 64
      OR lower(new.key_name) LIKE 'sk-%'
    THEN RAISE(ABORT, 'models.key_name is a name, never the secret')
  END;
END;
