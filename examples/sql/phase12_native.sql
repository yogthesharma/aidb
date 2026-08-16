-- Phase 12 proof is in-process bindings, not a new SQL surface.
-- Same file and same functions as `aidb sql`. No CLI spawn, no ctypes.
--
--   cargo build -p aidb-node -p aidb-python
--   node bindings/typescript/test.mjs
--   python3 bindings/python/test_open.py
--
-- Optional: confirm the faces still speak SQL against this file.
SELECT value FROM aidb_meta WHERE key = 'schema_version';
