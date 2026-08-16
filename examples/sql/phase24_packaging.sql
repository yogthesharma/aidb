-- Phase 24 demo: install the faces, then open the same file.
--   npm i ./bindings/typescript
--   pip install ./bindings/python
--   cargo install --path crates/aidb-cli
--   node -e "import { AI } from 'aidb'; const db = await AI.open('./app.db'); console.log(await db.query(\"SELECT value FROM aidb_meta WHERE key = 'schema_version'\"));"
--   python3 -c "from aidb import AI; db = AI.open('./app.db'); print(db.query(\"SELECT value FROM aidb_meta WHERE key = 'schema_version'\"))"
--   aidb sql ./app.db "SELECT value FROM aidb_meta WHERE key = 'schema_version'"
-- Native addons ship inside the npm package / wheel. Do not copy a dylib by hand.
-- Proof script: bash bindings/verify-packaging.sh
SELECT value FROM aidb_meta WHERE key = 'schema_version';
