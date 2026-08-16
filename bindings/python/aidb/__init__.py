"""Python face for AIDB. Loads the in-process PyO3 module. No second store."""

from __future__ import annotations

import importlib.machinery
import importlib.util
import json
import os
import shutil
import sys
from pathlib import Path
from typing import Any, Optional


RUNTIME = "pyo3"


def _native_names() -> list[str]:
    if sys.platform == "darwin":
        return [
            "aidb_native.abi3.so",
            "aidb_native.so",
            "libaidb_native.dylib",
        ]
    if os.name == "nt":
        return ["aidb_native.pyd", "aidb_native.dll", "libaidb_native.dll"]
    return ["aidb_native.abi3.so", "aidb_native.so", "libaidb_native.so"]


def _repo_target() -> Path | None:
    here = Path(__file__).resolve()
    try:
        repo = here.parents[3]
    except IndexError:
        return None
    if (repo / "crates" / "aidb-python" / "Cargo.toml").exists():
        return repo / "target"
    return None


def _cargo_native() -> Path | None:
    roots: list[Path] = []
    if os.environ.get("CARGO_TARGET_DIR"):
        roots.append(Path(os.environ["CARGO_TARGET_DIR"]))
    repo_target = _repo_target()
    if repo_target is not None:
        roots.append(repo_target)
    for root in roots:
        for folder in ("debug", "release"):
            for name in _native_names():
                candidate = root / folder / name
                if candidate.exists():
                    return candidate
    return None


def _ensure_extension(source: Path) -> Path:
    if source.suffix in {".so", ".pyd"}:
        return source
    dest = source.with_name("aidb_native.so")
    if not dest.exists() or source.stat().st_mtime > dest.stat().st_mtime:
        shutil.copy2(source, dest)
    return dest


def _load_from_path(path: Path) -> Any:
    path = _ensure_extension(path)
    loader = importlib.machinery.ExtensionFileLoader("aidb_native", str(path))
    spec = importlib.util.spec_from_loader("aidb_native", loader)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load aidb_native from {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _load_native() -> Any:
    env = os.environ.get("AIDB_PYTHON_LIB") or os.environ.get("AIDB_NATIVE")
    if env:
        path = Path(env)
        if path.exists():
            return _load_from_path(path)
    here = Path(__file__).resolve().parent
    for name in _native_names():
        candidate = here / name
        if candidate.exists():
            return _load_from_path(candidate)
    try:
        import aidb_native as installed

        if getattr(installed, "RUNTIME", None) == "pyo3":
            return installed
    except ImportError:
        pass
    cargo = _cargo_native()
    if cargo is not None:
        return _load_from_path(cargo)
    raise FileNotFoundError(
        "aidb native module not found. Install with: pip install aidb"
    )


_NATIVE = _load_native()
if getattr(_NATIVE, "RUNTIME", None) != "pyo3":
    raise ImportError("aidb native module is not the PyO3 addon")


def _sql_string(value: str) -> str:
    return "'" + str(value).replace("'", "''") + "'"


class Database:
    def __init__(self, inner: Any, path: str) -> None:
        self._inner = inner
        self.path = path

    def query(self, sql: str) -> dict[str, Any]:
        return self._inner.query(sql)

    def execute(self, sql: str) -> int:
        return int(self._inner.execute(sql))

    def session(self, name: Optional[str] = None) -> str:
        if name is None:
            result = self.query("SELECT aidb_session()")
        else:
            result = self.query(f"SELECT aidb_session({_sql_string(name)})")
        row = result["rows"][0]
        return "" if row[0] is None else str(row[0])

    def last_run_id(self) -> str:
        result = self.query("SELECT aidb_last_run_id()")
        row = result["rows"][0]
        return "" if row[0] is None else str(row[0])

    @property
    def documents(self) -> "_Documents":
        return _Documents(self)

    @property
    def memory(self) -> "_Memory":
        return _Memory(self)

    def search(self, query: str, limit: int = 5) -> dict[str, Any]:
        return self.query(
            "SELECT document_id, chunk_id, content, distance FROM aidb_search("
            f"{_sql_string(query)}, {int(limit)})"
        )

    @property
    def agent(self) -> "_Agent":
        return _Agent(self)

    @property
    def runs(self) -> "_Runs":
        return _Runs(self)

    def close(self) -> None:
        self._inner.close()

    def __enter__(self) -> "Database":
        return self

    def __exit__(self, *args: object) -> None:
        self.close()


class _Documents:
    def __init__(self, db: Database) -> None:
        self._db = db

    def insert(
        self,
        content: str,
        title: str = "",
        metadata: Optional[dict[str, Any]] = None,
    ) -> dict[str, str]:
        meta = json.dumps(metadata or {})
        result = self._db.query(
            "SELECT aidb_insert_document("
            f"{_sql_string(title)}, {_sql_string(content)}, {_sql_string(meta)})"
        )
        return {"id": str(result["rows"][0][0])}


class _Memory:
    def __init__(self, db: Database) -> None:
        self._db = db

    def insert(
        self,
        content: str,
        scope: Optional[str] = None,
        user_id: Optional[str] = None,
    ) -> dict[str, str]:
        key = scope if scope is not None else (f"user:{user_id}" if user_id else "")
        result = self._db.query(
            f"SELECT aidb_memory_insert({_sql_string(key)}, {_sql_string(content)})"
        )
        return {"id": str(result["rows"][0][0])}

    def search(
        self,
        query: str,
        limit: int = 5,
        scope: Optional[str] = None,
        user_id: Optional[str] = None,
    ) -> dict[str, Any]:
        key = scope if scope is not None else (f"user:{user_id}" if user_id else "")
        if key:
            return self._db.query(
                "SELECT document_id, content FROM aidb_memory_search("
                f"{_sql_string(query)}, {int(limit)}, {_sql_string(key)})"
            )
        return self._db.query(
            "SELECT document_id, content FROM aidb_memory_search("
            f"{_sql_string(query)}, {int(limit)})"
        )


class _Agent:
    def __init__(self, db: Database) -> None:
        self._db = db

    def run(
        self,
        instructions: str,
        goal: str,
        tools: Optional[list[str]] = None,
        max_steps: int = 4,
        k: int = 5,
        memory: Optional[str] = None,
        agents: Optional[list[dict[str, Any]]] = None,
        decide: bool = False,
        session: Optional[str] = None,
    ) -> dict[str, str]:
        spec = json.dumps(
            {
                "instructions": instructions,
                "goal": goal,
                "tools": tools or ["search", "generate"],
                "max_steps": max_steps,
                "k": k,
                "memory": memory,
                "agents": agents or [],
                "decide": decide,
                "session": session,
            }
        )
        result = self._db.query(f"SELECT aidb_agent({_sql_string(spec)})")
        row = result["rows"][0]
        return {"run_id": str(row[0]), "status": str(row[1]), "output": str(row[2])}


class _Runs:
    def __init__(self, db: Database) -> None:
        self._db = db

    def waiting(self) -> dict[str, Any]:
        return self._db.query(
            "SELECT id, kind, status, output_json FROM runs "
            "WHERE status IN ('awaiting_approval', 'suspended') "
            "ORDER BY created_at_ms"
        )

    def resume(
        self, run_id: str, decision: Optional[dict[str, Any]] = None
    ) -> dict[str, str]:
        payload = json.dumps(decision if decision is not None else {"approved": True})
        result = self._db.query(
            f"SELECT aidb_resume({_sql_string(run_id)}, {_sql_string(payload)})"
        )
        row = result["rows"][0]
        return {"run_id": str(row[0]), "status": str(row[1]), "output": str(row[2])}

    def events(self, run_id: str) -> dict[str, Any]:
        return self._db.query(
            "SELECT seq, kind, payload_json, created_at_ms FROM run_events "
            f"WHERE run_id = {_sql_string(run_id)} ORDER BY seq"
        )

    def tokens(self, run_id: str) -> dict[str, Any]:
        return self._db.query(
            "SELECT seq, json_extract(payload_json, '$.text') AS text, created_at_ms "
            f"FROM run_events WHERE run_id = {_sql_string(run_id)} AND kind = 'token' "
            "ORDER BY seq"
        )


class AI:
    runtime = RUNTIME

    @staticmethod
    def open(
        path: str,
        embedding: Optional[dict[str, Any]] = None,
    ) -> Database:
        embedding = embedding or {}
        inner = _NATIVE.open_db(
            path,
            embedding.get("provider"),
            embedding.get("model"),
            int(embedding["dimensions"]) if embedding.get("dimensions") else None,
        )
        return Database(inner, path)
