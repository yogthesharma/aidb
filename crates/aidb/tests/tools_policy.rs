//! Phase 13 / 19 / 21 / 9 contracts: capabilities are one catalog, tool calls are
//! runs, policy lives in the file, and approval is a run state.

mod common;

use common::*;

const EMAIL: &str = "{\"tools\":[{\"name\":\"send.email\",\"inputs\":{\"to\":\"string\"},\
    \"side_effect\":\"irreversible\",\"retry\":\"forbidden\"}]}";
const READER: &str = "{\"tools\":[{\"name\":\"github.read\",\"inputs\":{\"path\":\"string\"},\
    \"side_effect\":\"none\",\"retry\":\"safe\"}]}";

fn register(db: &aidb::Aidb, json: &str) {
    db.query(&format!("SELECT aidb_mcp_register('{}')", sql_escape(json)))
        .expect("register capability");
}

fn tool(db: &aidb::Aidb, name: &str, args: &str) -> aidb::QueryResult {
    db.query(&format!(
        "SELECT aidb_tool('{name}', '{}')",
        sql_escape(args)
    ))
    .expect("tool call")
}

#[test]
fn the_catalog_ships_the_builtin_operators_with_their_contracts() {
    let tmp = TempDb::new("cap-builtin");
    let db = tmp.open();
    let rows = db
        .query(
            "SELECT name, inputs, outputs, side_effect, retry, source, enabled
             FROM capabilities ORDER BY name",
        )
        .expect("capabilities");
    let names = column_values(&rows, "name");
    assert_eq!(names, vec!["generate", "search"], "{names:?}");
    for row in 0..rows.rows.len() {
        assert_eq!(cell(&rows, row, "side_effect"), "none");
        assert_eq!(cell(&rows, row, "retry"), "safe");
        assert_eq!(cell(&rows, row, "source"), "builtin");
        assert_eq!(cell(&rows, row, "enabled"), "1");
        assert!(cell(&rows, row, "inputs").starts_with('{'));
        assert!(cell(&rows, row, "outputs").starts_with('{'));
    }
}

#[test]
fn registering_a_tool_puts_it_in_the_same_catalog_with_its_metadata() {
    let tmp = TempDb::new("cap-register");
    let db = tmp.open();
    register(&db, READER);
    let row = db
        .query(
            "SELECT name, inputs, side_effect, retry, source FROM capabilities
             WHERE name = 'github.read'",
        )
        .expect("registered");
    assert_eq!(
        cell(&row, 0, "source"),
        "mcp",
        "MCP is an adapter, not a store"
    );
    assert_eq!(cell(&row, 0, "side_effect"), "none");
    assert_eq!(cell(&row, 0, "retry"), "safe");
    assert!(cell(&row, 0, "inputs").contains("path"));
    // The catalog is one table: builtins and adapters live side by side.
    assert_eq!(count(&db, "SELECT COUNT(*) FROM capabilities"), 3);
}

#[test]
fn the_catalog_rejects_nonsense_and_refuses_to_shadow_a_builtin() {
    let tmp = TempDb::new("cap-bad");
    let db = tmp.open();
    for (json, needle) in [
        (
            "{\"tools\":[{\"name\":\"x.y\",\"side_effect\":\"maybe\"}]}",
            "invalid side_effect",
        ),
        (
            "{\"tools\":[{\"name\":\"x.y\",\"retry\":\"sometimes\"}]}",
            "invalid retry",
        ),
        ("{\"tools\":[{\"inputs\":{}}]}", "name is required"),
        ("{\"tools\":[]}", "listed no tools"),
        ("{\"nope\":1}", "mcp register needs"),
        ("not json", "mcp register JSON"),
        (
            "{\"tools\":[{\"name\":\"search\"}]}",
            "cannot overwrite builtin capability",
        ),
    ] {
        assert_err_contains(
            db.query(&format!("SELECT aidb_mcp_register('{}')", sql_escape(json))),
            needle,
        );
    }
    assert_eq!(count(&db, "SELECT COUNT(*) FROM capabilities"), 2);
}

#[test]
fn invoking_a_tool_writes_a_tool_run_that_records_the_policy_it_ran_under() {
    let tmp = TempDb::new("tool-run");
    let db = tmp.open();
    register(&db, READER);
    let out = tool(&db, "github.read", "{\"path\":\"README.md\"}");
    assert_eq!(cell(&out, 0, "status"), "succeeded");
    let run_id = cell(&out, 0, "run_id");

    let row = db
        .query(&format!(
            "SELECT kind, status, input_json, output_json FROM runs WHERE id = '{run_id}'"
        ))
        .expect("run");
    assert_eq!(cell(&row, 0, "kind"), "tool");
    assert_eq!(cell(&row, 0, "status"), "succeeded");
    assert!(cell(&row, 0, "input_json").contains("README.md"));
    assert!(cell(&row, 0, "output_json").contains("README.md"));

    let events = column_values(
        &db.query(&format!(
            "SELECT kind, payload_json FROM run_events WHERE run_id = '{run_id}' ORDER BY seq"
        ))
        .expect("events"),
        "kind",
    );
    assert_eq!(events, vec!["policy", "succeeded"], "{events:?}");
    let policy = scalar(
        &db,
        &format!(
            "SELECT payload_json FROM run_events WHERE run_id = '{run_id}' AND kind = 'policy'"
        ),
    );
    assert!(policy.contains("read_only"), "{policy}");
}

#[test]
fn a_failing_tool_persists_its_error_on_the_run() {
    let tmp = TempDb::new("tool-fail");
    let db = tmp.open();
    register(
        &db,
        "{\"tools\":[{\"name\":\"http.get\",\"side_effect\":\"none\",\"retry\":\"safe\"}]}",
    );
    // The in-process tool runtime refuses to reach the network.
    assert_err_contains(
        db.query("SELECT aidb_tool('http.get', '{\"url\":\"https://example.com\"}')"),
        "aidb:// URLs",
    );
    let row = db
        .query("SELECT status, error FROM runs WHERE kind = 'tool' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1")
        .expect("run");
    assert_eq!(cell(&row, 0, "status"), "failed");
    assert!(
        cell(&row, 0, "error").contains("aidb://"),
        "{}",
        cell(&row, 0, "error")
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'tool' AND status = 'succeeded'"
        ),
        0
    );
}

#[test]
fn an_unknown_tool_is_a_clean_error_and_writes_no_run() {
    let tmp = TempDb::new("tool-unknown");
    let db = tmp.open();
    let before = count(&db, "SELECT COUNT(*) FROM runs");
    assert_err_contains(
        db.query("SELECT aidb_tool('ghost.tool', '{}')"),
        "unknown capability: ghost.tool",
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM runs"), before);
}

#[test]
fn a_denied_tool_never_executes() {
    let tmp = TempDb::new("policy-deny");
    let db = tmp.open();
    register(&db, READER);
    db.query("SELECT aidb_set_policy('{\"deny\":[\"github.read\"]}')")
        .expect("policy");
    assert_err_contains(
        db.query("SELECT aidb_tool('github.read', '{\"path\":\"README.md\"}')"),
        "deny-list",
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'tool'"),
        0,
        "a denied tool must not even open a run"
    );
}

#[test]
fn an_allow_list_excludes_everything_it_does_not_name() {
    let tmp = TempDb::new("policy-allow");
    let db = tmp.open();
    register(&db, READER);
    register(
        &db,
        "{\"tools\":[{\"name\":\"other.read\",\"side_effect\":\"none\",\"retry\":\"safe\"}]}",
    );
    db.query("SELECT aidb_set_policy('{\"allow\":[\"github.read\"]}')")
        .expect("policy");
    assert_eq!(
        cell(
            &tool(&db, "github.read", "{\"path\":\"README.md\"}"),
            0,
            "status"
        ),
        "succeeded"
    );
    assert_err_contains(
        db.query("SELECT aidb_tool('other.read', '{}')"),
        "not on the allow-list",
    );
}

#[test]
fn a_read_only_policy_blocks_every_side_effecting_capability() {
    let tmp = TempDb::new("policy-readonly");
    let db = tmp.open();
    register(&db, READER);
    register(
        &db,
        "{\"tools\":[{\"name\":\"cache.write\",\"side_effect\":\"reversible\",\"retry\":\"safe\"}]}",
    );
    db.query("SELECT aidb_set_policy('{\"read_only\":true}')")
        .expect("policy");
    // A pure read still works.
    assert_eq!(
        cell(
            &tool(&db, "github.read", "{\"path\":\"README.md\"}"),
            0,
            "status"
        ),
        "succeeded"
    );
    // Anything with a side effect does not, reversible or not.
    assert_err_contains(
        db.query("SELECT aidb_tool('cache.write', '{}')"),
        "read_only",
    );
}

#[test]
fn a_disabled_capability_is_denied_without_being_forgotten() {
    let tmp = TempDb::new("policy-disabled");
    let db = tmp.open();
    register(&db, READER);
    db.execute("UPDATE capabilities SET enabled = 0 WHERE name = 'github.read'")
        .expect("disable");
    assert_err_contains(
        db.query("SELECT aidb_tool('github.read', '{}')"),
        "denied (disabled)",
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM capabilities WHERE name = 'github.read'"
        ),
        1,
        "the row stays in the catalog"
    );
}

#[test]
fn the_policy_lives_in_the_file_and_survives_reopen() {
    let tmp = TempDb::new("policy-persist");
    {
        let db = tmp.open();
        db.query(
            "SELECT aidb_set_policy('strict', '{\"read_only\":true,\"deny\":[\"send.email\"],\"max_usd\":0.1}')",
        )
        .expect("policy");
    }
    let db = tmp.open();
    let json = scalar(&db, "SELECT aidb_get_policy()");
    let value: serde_json::Value = serde_json::from_str(&json).expect("policy json");
    assert_eq!(value["read_only"], true);
    assert_eq!(value["deny"][0], "send.email");
    assert_eq!(value["max_usd"], 0.1);
    assert_eq!(value["name"], "strict");
    // It is stored in the same file, in aidb_meta, not in a sidecar.
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM aidb_meta WHERE key = 'policy'"),
        1
    );
    assert!(
        std::fs::read_dir(tmp.dir())
            .expect("dir")
            .filter_map(|e| e.ok())
            .all(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.starts_with("app.db")
            }),
        "no policy sidecar file may appear"
    );
}

#[test]
fn the_policy_refuses_to_hold_secrets() {
    let tmp = TempDb::new("policy-secrets");
    let db = tmp.open();
    for json in [
        "{\"api_key\":\"sk-not-real\"}",
        "{\"secret\":\"nope\"}",
        "{\"token\":\"nope\"}",
        "{\"password\":\"nope\"}",
    ] {
        assert_err_contains(
            db.query(&format!("SELECT aidb_set_policy('{}')", sql_escape(json))),
            "secrets",
        );
    }
    let stored = db
        .query("SELECT value FROM aidb_meta")
        .expect("meta")
        .rows
        .iter()
        .map(|r| r[0].to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(!stored.contains("sk-not-real"), "{stored}");
}

#[test]
fn a_malformed_policy_is_rejected_field_by_field() {
    let tmp = TempDb::new("policy-invalid");
    let db = tmp.open();
    for (json, needle) in [
        ("[]", "must be a JSON object"),
        ("{\"max_usd\":\"free\"}", "max_usd must be a number"),
        ("{\"max_ms\":\"soon\"}", "max_ms must be an integer"),
        ("{\"read_only\":\"yes\"}", "read_only must be a boolean"),
        ("{\"deny\":[1]}", "entries must be strings"),
        ("{oops}", "policy JSON"),
    ] {
        assert_err_contains(
            db.query(&format!("SELECT aidb_set_policy('{}')", sql_escape(json))),
            needle,
        );
    }
    assert!(scalar(&db, "SELECT aidb_get_policy()").contains("read_only"));
}

#[test]
fn a_goal_constraint_can_only_tighten_the_stored_budget() {
    let tmp = TempDb::new("policy-overlay");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    db.query("SELECT aidb_set_policy('{\"max_usd\":0.5}')")
        .expect("policy");

    let tighter = scalar(
        &db,
        "EXPLAIN TASK summarize\nDATA documents\nCONSTRAINTS budget $0.01\nGOAL How do refunds work?",
    );
    assert!(
        tighter.contains("max_usd=0.01"),
        "the tighter bound wins:\n{tighter}"
    );
    let looser = scalar(
        &db,
        "EXPLAIN TASK summarize\nDATA documents\nCONSTRAINTS budget $99\nGOAL How do refunds work?",
    );
    assert!(
        looser.contains("max_usd=0.5"),
        "a goal must not loosen the stored policy:\n{looser}"
    );
}

#[test]
fn an_irreversible_tool_parks_for_approval_instead_of_running() {
    let tmp = TempDb::new("hitl-park");
    let db = tmp.open();
    register(&db, EMAIL);
    let parked = tool(
        &db,
        "send.email",
        "{\"to\":\"user@example.com\",\"subject\":\"hi\"}",
    );
    let run_id = cell(&parked, 0, "run_id");
    assert_eq!(cell(&parked, 0, "status"), "awaiting_approval");

    let row = db
        .query(&format!(
            "SELECT kind, status, output_json FROM runs WHERE id = '{run_id}'"
        ))
        .expect("run");
    assert_eq!(cell(&row, 0, "kind"), "tool");
    assert_eq!(
        cell(&row, 0, "status"),
        "awaiting_approval",
        "approval is a run state, visible through SQL"
    );
    // What is recorded is the question, not a result: the handler never ran.
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT json_valid(output_json) FROM runs WHERE id = '{run_id}'")
        ),
        "1"
    );
    assert_eq!(
        scalar(
            &db,
            &format!(
                "SELECT json_extract(output_json, '$.message') FROM runs WHERE id = '{run_id}'"
            )
        ),
        "approve irreversible tool send.email"
    );
    assert!(
        !cell(&row, 0, "output_json").contains("queued"),
        "the tool must not have executed before the human answered: {}",
        cell(&row, 0, "output_json")
    );
    // Approval is not a new operator or table.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE '%approv%'"
        ),
        0
    );
}

#[test]
fn approving_a_parked_tool_runs_it_exactly_once() {
    let tmp = TempDb::new("hitl-approve");
    let db = tmp.open();
    register(&db, EMAIL);
    let run_id = cell(
        &tool(
            &db,
            "send.email",
            "{\"to\":\"user@example.com\",\"subject\":\"hi\"}",
        ),
        0,
        "run_id",
    );

    let resumed = db
        .query(&format!(
            "SELECT aidb_resume('{run_id}', '{{\"approved\":true}}')"
        ))
        .expect("resume");
    assert_eq!(cell(&resumed, 0, "status"), "succeeded");
    let row = db
        .query(&format!(
            "SELECT status, output_json FROM runs WHERE id = '{run_id}'"
        ))
        .expect("run");
    assert_eq!(cell(&row, 0, "status"), "succeeded");
    assert!(cell(&row, 0, "output_json").contains("user@example.com"));
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'tool'"),
        1,
        "approval resumes the parked run, it does not start a second one"
    );
    let events = column_values(
        &db.query(&format!(
            "SELECT kind FROM run_events WHERE run_id = '{run_id}' ORDER BY seq"
        ))
        .expect("events"),
        "kind",
    );
    assert_eq!(
        events,
        vec!["policy", "awaiting_approval", "resumed", "succeeded"],
        "{events:?}"
    );
}

#[test]
fn rejecting_a_parked_tool_cancels_it_and_never_executes_it() {
    let tmp = TempDb::new("hitl-reject");
    let db = tmp.open();
    register(&db, EMAIL);
    let run_id = cell(
        &tool(&db, "send.email", "{\"to\":\"user@example.com\"}"),
        0,
        "run_id",
    );
    let out = db
        .query(&format!(
            "SELECT aidb_resume('{run_id}', '{{\"approved\":false}}')"
        ))
        .expect("reject");
    assert_eq!(cell(&out, 0, "status"), "cancelled");
    let row = db
        .query(&format!(
            "SELECT status, error, output_json FROM runs WHERE id = '{run_id}'"
        ))
        .expect("run");
    assert_eq!(cell(&row, 0, "status"), "cancelled");
    assert_eq!(cell(&row, 0, "error"), "rejected");
    assert_eq!(
        cell(&row, 0, "output_json"),
        "",
        "a rejected tool produces nothing"
    );
}

#[test]
fn resume_rejects_input_that_is_not_a_decision() {
    let tmp = TempDb::new("hitl-bad-resume");
    let db = tmp.open();
    register(&db, EMAIL);
    let run_id = cell(&tool(&db, "send.email", "{\"to\":\"a@b.c\"}"), 0, "run_id");

    assert_err_contains(
        db.query(&format!("SELECT aidb_resume('{run_id}', 'not json')")),
        "resume JSON",
    );
    assert_err_contains(
        db.query(&format!("SELECT aidb_resume('{run_id}', '{{}}')")),
        "requires {\"approved\":true}",
    );
    assert_err_contains(
        db.query("SELECT aidb_resume('run_does_not_exist', '{\"approved\":true}')"),
        "unknown run",
    );
    // Still parked, untouched.
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT status FROM runs WHERE id = '{run_id}'")
        ),
        "awaiting_approval"
    );
    // And a run that is not waiting cannot be resumed.
    db.query(&format!(
        "SELECT aidb_resume('{run_id}', '{{\"approved\":true}}')"
    ))
    .expect("approve");
    assert_err_contains(
        db.query(&format!(
            "SELECT aidb_resume('{run_id}', '{{\"approved\":true}}')"
        )),
        "not awaiting_approval or suspended",
    );
}

#[test]
fn a_parked_run_can_be_approved_after_the_process_restarts() {
    let tmp = TempDb::new("hitl-restart");
    let run_id = {
        let db = tmp.open();
        register(&db, EMAIL);
        cell(&tool(&db, "send.email", "{\"to\":\"a@b.c\"}"), 0, "run_id")
    };
    // A fresh process sees the parked run through SQL and can approve it.
    let db = tmp.open();
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE status = 'awaiting_approval'"
        ),
        1,
        "reopen must not sweep a waiting run into failed"
    );
    let out = db
        .query(&format!(
            "SELECT aidb_resume('{run_id}', '{{\"approved\":true}}')"
        ))
        .expect("resume after restart");
    assert_eq!(cell(&out, 0, "status"), "succeeded");
    assert!(scalar(
        &db,
        &format!("SELECT output_json FROM runs WHERE id = '{run_id}'")
    )
    .contains("a@b.c"));
}

#[test]
fn a_workflow_approval_resumes_at_the_operator_that_was_waiting() {
    let tmp = TempDb::new("hitl-workflow");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    let spec = "{\"then\":[{\"search\":{\"query\":\"How do refunds work?\",\"k\":5}},\
        {\"approve\":{\"message\":\"Send this answer?\"}},{\"generate\":{\"prompt\":\"Draft the reply\"}}]}";
    let parked = db
        .query(&format!("SELECT aidb_workflow('{}')", sql_escape(spec)))
        .expect("workflow");
    let run_id = cell(&parked, 0, "run_id");
    assert_eq!(cell(&parked, 0, "status"), "awaiting_approval");
    assert_eq!(cell(&parked, 0, "output"), "Send this answer?");
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT json_valid(output_json) FROM runs WHERE id = '{run_id}'")
        ),
        "1"
    );
    assert_eq!(
        scalar(
            &db,
            &format!(
                "SELECT json_extract(output_json, '$.message') FROM runs WHERE id = '{run_id}'"
            )
        ),
        "Send this answer?"
    );

    // The search ran, the generate did not.
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{run_id}' AND kind = 'search'")
        ),
        1
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM runs WHERE parent_id = '{run_id}' AND kind = 'generate'"
            )
        ),
        0,
        "the operator after the pause must wait"
    );

    let resumed = db
        .query(&format!(
            "SELECT aidb_resume('{run_id}', '{{\"approved\":true}}')"
        ))
        .expect("resume");
    assert_eq!(cell(&resumed, 0, "status"), "succeeded");
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{run_id}' AND kind = 'search'")
        ),
        1,
        "resume must not repeat the operator that already committed"
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM runs WHERE parent_id = '{run_id}' AND kind = 'generate'"
            )
        ),
        1,
        "resume continues at the waiting operator"
    );
}

#[test]
fn an_agent_that_wants_an_irreversible_tool_still_needs_a_human() {
    let tmp = TempDb::new("hitl-agent");
    let db = tmp.open();
    register(&db, EMAIL);
    // Even an explicit allow-list does not bypass approval for an irreversible tool.
    db.query("SELECT aidb_set_policy('{\"allow\":[\"send.email\",\"search\",\"generate\"]}')")
        .expect("policy");
    let spec = "{\"instructions\":\"Email the customer\",\"goal\":\"Send the refund summary\",\
        \"tools\":[\"send.email\"],\"max_steps\":1}";
    let out = db
        .query(&format!("SELECT aidb_agent('{}')", sql_escape(spec)))
        .expect("agent");
    let run_id = cell(&out, 0, "run_id");
    assert_eq!(cell(&out, 0, "status"), "awaiting_approval");
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT status FROM runs WHERE id = '{run_id}'")
        ),
        "awaiting_approval"
    );
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT json_valid(output_json) FROM runs WHERE id = '{run_id}'")
        ),
        "1"
    );
    assert_eq!(
        scalar(
            &db,
            &format!(
                "SELECT json_extract(output_json, '$.message') FROM runs WHERE id = '{run_id}'"
            )
        ),
        "approve irreversible tool send.email"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'tool' AND status = 'succeeded'"
        ),
        0,
        "nothing irreversible happened before approval"
    );

    let resumed = db
        .query(&format!(
            "SELECT aidb_resume('{run_id}', '{{\"approved\":true}}')"
        ))
        .expect("resume");
    assert_eq!(cell(&resumed, 0, "status"), "succeeded");
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'tool' AND status = 'succeeded'"
        ),
        1,
        "after approval the tool runs exactly once"
    );
}

#[test]
fn the_catalog_and_its_tools_still_work_after_a_reopen() {
    let tmp = TempDb::new("cap-reopen");
    {
        let db = tmp.open();
        register(&db, READER);
        tool(&db, "github.read", "{\"path\":\"README.md\"}");
    }
    let db = tmp.open();
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM capabilities WHERE source = 'mcp'"
        ),
        1,
        "the catalog is durable, not session state"
    );
    let out = tool(&db, "github.read", "{\"path\":\"CHANGELOG.md\"}");
    assert_eq!(cell(&out, 0, "status"), "succeeded");
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'tool' AND status = 'succeeded'"
        ),
        2,
        "both invocations are recorded in the same run store"
    );
}

#[test]
fn an_approved_workflow_can_run_the_irreversible_tool_that_follows() {
    let tmp = TempDb::new("hitl-workflow-tool");
    let db = tmp.open();
    register(&db, EMAIL);
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    let out = db
        .query(
            "SELECT aidb_workflow('{\"then\":[{\"search\":{\"query\":\"refunds\",\"k\":3}},\
             {\"approve\":{\"message\":\"send this?\"}},{\"tool\":\"send.email\"}]}')",
        )
        .expect("workflow");
    let run_id = cell(&out, 0, "run_id");
    assert_eq!(cell(&out, 0, "status"), "awaiting_approval");
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'tool'"),
        0,
        "nothing irreversible happened before approval"
    );

    let resumed = db
        .query(&format!(
            "SELECT aidb_resume('{run_id}', '{{\"approved\":true}}')"
        ))
        .expect("resume");
    assert_eq!(cell(&resumed, 0, "status"), "succeeded");
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM runs WHERE parent_id = '{run_id}' AND kind = 'tool' AND status = 'succeeded'"
            )
        ),
        1,
        "the approval covers the tool that follows it"
    );
}

#[test]
fn a_workflow_tool_without_an_approve_step_still_fails_closed() {
    let tmp = TempDb::new("hitl-workflow-no-approve");
    let db = tmp.open();
    register(&db, EMAIL);
    assert_err_contains(
        db.query("SELECT aidb_workflow('{\"tool\":\"send.email\"}')"),
        "irreversible and needs approval",
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'tool' AND status = 'succeeded'"
        ),
        0
    );
}
