//! Parse TASK / DATA / CONSTRAINTS / GOAL. Compile happens in aidb-ir.

use aidb_core::{Error, Result};
use aidb_ir::GoalSpec;

pub fn parse_aidb_task(sql: &str) -> Option<String> {
    let (_, _, args) = crate::parse_call(sql, "aidb_task")?;
    if args.len() != 1 {
        return None;
    }
    Some(args[0].clone())
}

pub fn parse_goal_sql(sql: &str) -> Result<GoalSpec> {
    if let Some(inner) = parse_aidb_task(sql) {
        return parse_goal(&inner);
    }
    parse_goal(sql)
}

pub fn looks_like_goal(sql: &str) -> bool {
    parse_aidb_task(sql).is_some() || starts_with_task(sql)
}

fn starts_with_task(sql: &str) -> bool {
    let trimmed = sql.trim_start();
    trimmed.len() >= 4 && trimmed[..4].eq_ignore_ascii_case("task")
}

pub fn parse_goal(text: &str) -> Result<GoalSpec> {
    let mut task = None;
    let mut data = Vec::new();
    let mut constraints = Vec::new();
    let mut goal = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = strip_prefix(line, "task") {
            task = Some(rest);
        } else if let Some(rest) = strip_prefix(line, "data") {
            data = split_csv(&rest);
        } else if let Some(rest) = strip_prefix(line, "constraints") {
            constraints = split_csv(&rest);
        } else if let Some(rest) = strip_prefix(line, "goal") {
            goal = Some(rest);
        } else {
            return Err(Error::usage(format!("unknown goal line: {line}")));
        }
    }
    let task = task.ok_or_else(|| Error::usage("TASK is required"))?;
    let goal = goal.ok_or_else(|| Error::usage("GOAL is required"))?;
    let mut spec = GoalSpec {
        task,
        data,
        goal,
        read_only: false,
        max_usd: None,
        max_ms: None,
        k: 5,
        source: text.trim().to_string(),
    };
    apply_constraints(&mut spec, &constraints)?;
    Ok(spec)
}

fn apply_constraints(spec: &mut GoalSpec, items: &[String]) -> Result<()> {
    for item in items {
        let lower = item.to_ascii_lowercase();
        if lower == "read_only" || lower == "readonly" || lower == "read-only" {
            spec.read_only = true;
            continue;
        }
        if let Some(rest) = lower.strip_prefix("budget") {
            spec.max_usd = Some(parse_usd(rest.trim())?);
            continue;
        }
        if let Some(rest) = lower.strip_prefix("timeout") {
            spec.max_ms = Some(parse_timeout(rest.trim())?);
            continue;
        }
        if let Some(rest) = lower.strip_prefix("top") {
            spec.k = parse_k(rest.trim())?;
            continue;
        }
        if let Some(rest) = lower
            .strip_prefix("limit")
            .or_else(|| lower.strip_prefix('k'))
        {
            spec.k = parse_k(rest.trim())?;
            continue;
        }
        return Err(Error::usage(format!("unknown constraint: {item}")));
    }
    Ok(())
}

fn parse_usd(raw: &str) -> Result<f64> {
    let raw = raw.trim().trim_start_matches('$');
    raw.parse::<f64>()
        .map_err(|_| Error::usage(format!("invalid budget: {raw}")))
}

fn parse_timeout(raw: &str) -> Result<u64> {
    let raw = raw.trim().replace(' ', "");
    if let Some(n) = raw.strip_suffix("ms") {
        return n
            .parse()
            .map_err(|_| Error::usage(format!("invalid timeout: {raw}")));
    }
    if let Some(n) = raw.strip_suffix("min").or_else(|| raw.strip_suffix('m')) {
        let mins: u64 = n
            .parse()
            .map_err(|_| Error::usage(format!("invalid timeout: {raw}")))?;
        return Ok(mins.saturating_mul(60_000));
    }
    if let Some(n) = raw.strip_suffix('s') {
        let secs: u64 = n
            .parse()
            .map_err(|_| Error::usage(format!("invalid timeout: {raw}")))?;
        return Ok(secs.saturating_mul(1000));
    }
    Err(Error::usage(format!("invalid timeout: {raw}")))
}

fn parse_k(raw: &str) -> Result<i64> {
    raw.parse::<i64>()
        .map(|k| k.max(1))
        .map_err(|_| Error::usage(format!("invalid k: {raw}")))
}

fn strip_prefix(line: &str, key: &str) -> Option<String> {
    if line.len() < key.len() || !line[..key.len()].eq_ignore_ascii_case(key) {
        return None;
    }
    let rest = line[key.len()..].trim_start();
    if let Some(stripped) = rest.strip_prefix(':') {
        return Some(stripped.trim().to_string());
    }
    if rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_whitespace()) {
        return Some(rest.trim().to_string());
    }
    // TASK investigate_incident (space already consumed by trim on original...
    // after key, if next is ident, accept.
    if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Some(rest.to_string());
    }
    None
}

fn split_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_phase_example() {
        let spec = parse_goal(
            "TASK investigate_incident\n\
             DATA logs, deployments\n\
             CONSTRAINTS read_only, budget $1, timeout 5m\n\
             GOAL identify_root_cause",
        )
        .expect("parse");
        assert_eq!(spec.task, "investigate_incident");
        assert_eq!(spec.data, ["logs", "deployments"]);
        assert!(spec.read_only);
        assert_eq!(spec.max_usd, Some(1.0));
        assert_eq!(spec.max_ms, Some(300_000));
        assert_eq!(spec.goal, "identify_root_cause");
    }

    #[test]
    fn parses_aidb_task_call() {
        let inner =
            parse_aidb_task("SELECT aidb_task('TASK summarize\nGOAL How do refunds work?');")
                .expect("call");
        let spec = parse_goal(&inner).expect("goal");
        assert_eq!(spec.task, "summarize");
        assert_eq!(spec.goal, "How do refunds work?");
    }
}
