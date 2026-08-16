# aidb

TypeScript face for AIDB. `npm i aidb`, then:

```ts
import { AI } from "aidb";
const db = await AI.open("./app.db");
await db.query("SELECT value FROM aidb_meta WHERE key = 'schema_version'");
```

The napi addon ships inside this package. Do not copy `aidb.node` by hand. See the repository README.
