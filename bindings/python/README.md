# aidb

Python face for AIDB. `pip install aidb`, then:

```python
from aidb import AI
db = AI.open("./app.db")
db.query("SELECT value FROM aidb_meta WHERE key = 'schema_version'")
```

The PyO3 module (`aidb_native`) ships inside the wheel. Do not copy a dylib by hand. See the repository README.
