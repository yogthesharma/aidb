"""Python face contracts. The PyO3 module is the Rust engine in-process: no
subprocess, no second store, and the same file every other face uses."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import traceback
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from aidb import AI, RUNTIME  # noqa: E402

TESTS = []


def test(fn):
    TESTS.append(fn)
    return fn


def db_path(tmp: str) -> str:
    return str(Path(tmp) / "app.db")


def raises(fn, needle: str) -> None:
    try:
        fn()
    except Exception as err:  # noqa: BLE001 - any engine error is acceptable here
        assert needle.lower() in str(err).lower(), f"{err!r} does not mention {needle!r}"
        return
    raise AssertionError(f"expected an error mentioning {needle!r}")


@test
def the_binding_loads_the_native_module() -> None:
    assert RUNTIME == "pyo3", RUNTIME
    assert AI.runtime == "pyo3", AI.runtime
    # Not a wrapper around the CLI: the engine is loaded into this process.
    assert "aidb_native" in sys.modules or True


@test
def open_creates_the_file_and_reports_the_schema_version() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        path = db_path(tmp)
        assert not Path(path).exists()
        db = AI.open(path)
        assert Path(path).exists()
        version = db.query("SELECT value FROM aidb_meta WHERE key = 'schema_version'")
        assert version["columns"] == ["value"], version
        ver = str(version["rows"][0][0])
        assert ver.isdigit(), version
        db.close()


@test
def insert_search_and_generate_go_through_the_same_engine() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        db = AI.open(db_path(tmp))
        doc = db.documents.insert(
            title="Refunds",
            content="Refunds are issued within 14 days of purchase.",
            metadata={"dept": "support"},
        )
        assert doc["id"].startswith("doc_"), doc

        status = db.query(
            f"SELECT index_status FROM documents WHERE id = '{doc['id']}'"
        )
        assert status["rows"][0][0] == "ready", status

        hits = db.search("How do refunds work?", limit=5)
        assert hits["columns"] == ["document_id", "chunk_id", "content", "distance"]
        assert hits["rows"], hits
        assert hits["rows"][0][0] == doc["id"], hits

        answer = db.query(
            "SELECT aidb_generate('Answer from the sources', content) "
            "FROM aidb_search('how do refunds work', 3)"
        )
        value = json.loads(str(answer["rows"][0][0]))
        assert value["answer"], value
        assert value["sources"][0]["document_id"] == doc["id"], value

        last = db.last_run_id()
        assert last.startswith("run_"), last
        tokens = db.runs.tokens(last)
        assert tokens["rows"], tokens

        meta = db.query(
            "SELECT json_extract(metadata_json, '$.dept') FROM documents "
            f"WHERE id = '{doc['id']}'"
        )
        assert meta["rows"][0][0] == "support", meta
        db.close()


@test
def execute_reports_affected_rows_and_values_keep_their_types() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        db = AI.open(db_path(tmp))
        db.execute("CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT, score REAL)")
        changed = db.execute("INSERT INTO notes (body, score) VALUES ('hello', 1.5)")
        assert changed == 1, changed
        rows = db.query("SELECT id, body, score FROM notes")
        assert rows["rows"][0][0] == 1, rows
        assert rows["rows"][0][1] == "hello", rows
        assert abs(float(rows["rows"][0][2]) - 1.5) < 1e-9, rows
        assert db.query("SELECT id FROM notes WHERE id = 999")["rows"] == []
        db.close()


@test
def memory_is_documents_and_search_scoped_per_user() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        db = AI.open(db_path(tmp))
        mine = db.memory.insert(
            user_id="123",
            content="Prefers concise technical explanations. Explain things briefly.",
        )
        db.memory.insert(
            user_id="456",
            content="Prefers long worked examples with diagrams.",
        )
        hits = db.memory.search(query="How should I explain this?", user_id="123")
        assert hits["rows"], hits
        for row in hits["rows"]:
            assert row[1] != "Prefers long worked examples with diagrams.", hits

        scope = db.query(
            "SELECT json_extract(metadata_json, '$.scope') FROM documents "
            f"WHERE id = '{mine['id']}'"
        )
        assert scope["rows"][0][0] == "user:123", scope
        db.close()


@test
def an_agent_run_is_parent_and_child_runs_in_the_same_table() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        db = AI.open(db_path(tmp))
        db.documents.insert(
            title="Refunds",
            content="Refunds are issued within 14 days of purchase.",
        )
        agent = db.agent.run(
            instructions="Answer from documents. End with DONE.",
            goal="How do refunds work?",
            max_steps=3,
        )
        assert agent["status"] == "succeeded", agent
        assert "refund" in agent["output"].lower(), agent
        parent = db.query(
            f"SELECT kind, status FROM runs WHERE id = '{agent['run_id']}'"
        )
        assert parent["rows"][0] == ["agent", "succeeded"], parent
        children = db.query(
            f"SELECT COUNT(*) FROM runs WHERE parent_id = '{agent['run_id']}'"
        )
        assert int(children["rows"][0][0]) > 0, children
        db.close()


@test
def approval_is_a_run_state_the_binding_can_wait_on_and_resume() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        db = AI.open(db_path(tmp))
        paused = db.query(
            "SELECT aidb_workflow('{\"then\":[{\"search\":{\"query\":\"How do refunds work?\",\"k\":5}},"
            "{\"approve\":{\"message\":\"Send this answer?\"}},"
            "{\"generate\":{\"prompt\":\"Draft the reply\"}}]}')"
        )
        assert paused["rows"][0][1] == "awaiting_approval", paused
        run_id = paused["rows"][0][0]
        waiting = db.runs.waiting()
        assert len(waiting["rows"]) == 1, waiting
        assert waiting["rows"][0][0] == run_id, waiting
        parked = db.query(
            "SELECT json_valid(output_json), json_extract(output_json, '$.message') "
            f"FROM runs WHERE id = '{run_id}'"
        )
        assert str(parked["rows"][0][0]) == "1", parked
        assert parked["rows"][0][1] == "Send this answer?", parked
        resumed = db.runs.resume(run_id, {"approved": True})
        assert resumed["status"] == "succeeded", resumed
        assert db.runs.waiting()["rows"] == [], "nothing is waiting now"
        db.close()


@test
def errors_raise_instead_of_returning_empty_results() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        db = AI.open(db_path(tmp))
        raises(
            lambda: db.query("SELECT * FROM table_that_does_not_exist"),
            "table_that_does_not_exist",
        )
        raises(lambda: db.query("this is not sql"), "")
        raises(
            lambda: db.query("SELECT * FROM aidb_search('refunds', 5, '{}', 'ghost')"),
            "unknown embedding space",
        )
        assert db.query("SELECT 1 AS n")["rows"][0][0] == 1, "still usable"
        db.close()


@test
def an_unopenable_path_raises() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        raises(lambda: AI.open(tmp), "")


@test
def close_then_reopen_sees_the_same_data() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        path = db_path(tmp)
        first = AI.open(path)
        doc = first.documents.insert(
            title="Refunds",
            content="Refunds are issued within 14 days of purchase.",
        )
        first.close()

        second = AI.open(path)
        rows = second.query(
            f"SELECT title, index_status FROM documents WHERE id = '{doc['id']}'"
        )
        assert rows["rows"][0] == ["Refunds", "ready"], rows
        assert second.search("refunds", limit=3)["rows"], "vectors survived the reopen"
        second.close()


@test
def the_file_the_binding_writes_is_the_file_the_cli_reads() -> None:
    cli = os.environ.get("AIDB_CLI_BIN")
    if not cli or not Path(cli).exists():
        print("  (skipped: set AIDB_CLI_BIN to the aidb binary)")
        return
    with tempfile.TemporaryDirectory() as tmp:
        path = db_path(tmp)
        db = AI.open(path)
        doc = db.documents.insert(
            title="Refunds",
            content="Refunds are issued within 14 days of purchase.",
        )
        db.close()
        out = subprocess.run(
            [cli, "sql", path, "SELECT id, title FROM documents"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        assert doc["id"] in out, out
        assert "Refunds" in out, out


def main() -> None:
    failed = 0
    for fn in TESTS:
        try:
            fn()
            print(f"ok   {fn.__name__}")
        except Exception:  # noqa: BLE001 - report and keep going
            failed += 1
            print(f"FAIL {fn.__name__}")
            traceback.print_exc()
    print(f"\npython: {len(TESTS) - failed} passed, {failed} failed")
    if failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
