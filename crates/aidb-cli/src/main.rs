use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("sql") => {
            let Some(path) = args.next() else {
                return usage();
            };
            let Some(sql) = args.next() else {
                return usage();
            };
            if args.next().is_some() {
                return usage();
            }
            run_sql(&path, &sql)
        }
        Some("runs") => {
            let mut path = None;
            let mut waiting = false;
            for arg in args {
                if arg == "--waiting" {
                    waiting = true;
                } else if path.is_none() {
                    path = Some(arg);
                } else {
                    return usage();
                }
            }
            let Some(path) = path else {
                return usage();
            };
            run_list(&path, waiting)
        }
        Some("serve") => {
            let mut path = None;
            let mut bind = None;
            while let Some(arg) = args.next() {
                if arg == "--bind" {
                    let Some(value) = args.next() else {
                        return usage();
                    };
                    bind = Some(value);
                } else if let Some(value) = arg.strip_prefix("--bind=") {
                    bind = Some(value.to_string());
                } else if path.is_none() && !arg.starts_with('-') {
                    path = Some(arg);
                } else {
                    return usage();
                }
            }
            let Some(path) = path else {
                return usage();
            };
            let bind = bind.unwrap_or_else(aidb_server::bind_from_env);
            run_serve(&path, &bind)
        }
        _ => usage(),
    }
}

fn run_sql(path: &str, sql: &str) -> ExitCode {
    let db = match aidb::open(path) {
        Ok(db) => db,
        Err(err) => {
            eprintln!("aidb: {err}");
            return ExitCode::FAILURE;
        }
    };

    let trimmed = sql.trim_start();
    let is_query = starts_with_ignore_ascii(trimmed, "select")
        || starts_with_ignore_ascii(trimmed, "pragma")
        || starts_with_ignore_ascii(trimmed, "with")
        || starts_with_ignore_ascii(trimmed, "explain")
        || starts_with_ignore_ascii(trimmed, "search")
        || starts_with_ignore_ascii(trimmed, "task");

    if is_query {
        match db.query(sql) {
            Ok(result) => {
                if let Err(err) = maybe_drain(&db, sql) {
                    eprintln!("aidb: {err}");
                    return ExitCode::FAILURE;
                }
                print!("{}", result.to_tsv());
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("aidb: {err}");
                ExitCode::FAILURE
            }
        }
    } else {
        match db.execute(sql) {
            Ok(_) => {
                if let Err(err) = maybe_drain(&db, sql) {
                    eprintln!("aidb: {err}");
                    return ExitCode::FAILURE;
                }
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("aidb: {err}");
                ExitCode::FAILURE
            }
        }
    }
}

fn run_list(path: &str, waiting: bool) -> ExitCode {
    let db = match aidb::open(path) {
        Ok(db) => db,
        Err(err) => {
            eprintln!("aidb: {err}");
            return ExitCode::FAILURE;
        }
    };
    let sql = if waiting {
        "SELECT id, kind, status, output_json, created_at_ms
         FROM runs
         WHERE status IN ('awaiting_approval', 'suspended')
         ORDER BY created_at_ms DESC
         LIMIT 50"
    } else {
        "SELECT id, kind, status, error, created_at_ms
         FROM runs
         ORDER BY created_at_ms DESC
         LIMIT 50"
    };
    match db.query(sql) {
        Ok(result) => {
            print!("{}", result.to_tsv());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("aidb: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run_serve(path: &str, bind: &str) -> ExitCode {
    match aidb_server::serve(path, bind) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("aidb: {err}");
            ExitCode::FAILURE
        }
    }
}

fn maybe_drain(db: &aidb::Aidb, sql: &str) -> aidb::Result<()> {
    let lower = sql.to_ascii_lowercase();
    if lower.contains("insert") || lower.contains("aidb_insert_document") {
        db.drain_index(std::time::Duration::from_secs(60))?;
    }
    Ok(())
}

fn starts_with_ignore_ascii(sql: &str, keyword: &str) -> bool {
    sql.get(..keyword.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(keyword))
}

fn usage() -> ExitCode {
    eprintln!("Usage: aidb sql <database> <sql>");
    eprintln!("       aidb runs <database> [--waiting]");
    eprintln!("       aidb serve <database> [--bind 127.0.0.1:8080]");
    ExitCode::FAILURE
}
