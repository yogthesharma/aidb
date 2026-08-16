use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut path = None;
    let mut bind = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--bind" {
            bind = args.next();
            if bind.is_none() {
                return usage();
            }
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
    match aidb_server::serve(&path, &bind) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("aidb-server: {err}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> ExitCode {
    eprintln!("Usage: aidb-server <database> [--bind 127.0.0.1:8080]");
    ExitCode::FAILURE
}
