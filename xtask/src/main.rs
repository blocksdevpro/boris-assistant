use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use boris_agent::{summarize_traces, TurnTrace};

fn main() {
    if let Err(error) = run(env::args().skip(1).collect()) {
        eprintln!("xtask: {error}");
        std::process::exit(2);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(usage());
    };
    match command {
        "trace-report" => trace_report(&args[1..]),
        "help" | "--help" | "-h" => {
            println!("{}", usage());
            Ok(())
        }
        other => Err(format!("unknown command `{other}`\n{}", usage())),
    }
}

fn trace_report(args: &[String]) -> Result<(), String> {
    let json = args.iter().any(|arg| arg == "--json");
    let path = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(PathBuf::from)
        .unwrap_or_else(default_trace_path);
    let input =
        fs::read_to_string(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let (traces, skipped) = parse_jsonl(&input);
    if traces.is_empty() {
        return Err(format!("no valid turn traces in {}", path.display()));
    }
    let summary = summarize_traces(&traces);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&summary)
                .map_err(|error| format!("serialize report: {error}"))?
        );
    } else {
        println!("Turn trace report: {}", path.display());
        println!("turns                 {}", summary.count);
        println!("generation p50        {} ms", summary.generation_p50_ms);
        println!("generation p95        {} ms", summary.generation_p95_ms);
        println!("playback p50          {} ms", summary.playback_p50_ms);
        println!("playback p95          {} ms", summary.playback_p95_ms);
        if skipped > 0 {
            println!("malformed lines       {skipped} (skipped)");
        }
    }
    Ok(())
}

fn parse_jsonl(input: &str) -> (Vec<TurnTrace>, usize) {
    let mut traces = Vec::new();
    let mut skipped = 0;
    for line in input.lines().map(str::trim).filter(|line| !line.is_empty()) {
        match serde_json::from_str(line) {
            Ok(trace) => traces.push(trace),
            Err(_) => skipped += 1,
        }
    }
    (traces, skipped)
}

fn default_trace_path() -> PathBuf {
    if let Some(root) = env::var_os("BORIS_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(root).join("traces").join("turns.jsonl");
    }
    let home = env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    home.join(".boris").join("traces").join("turns.jsonl")
}

fn usage() -> String {
    "usage: cargo xtask trace-report [--json] [path-to-turns.jsonl]".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_keeps_valid_rows_and_skips_torn_tail() {
        let trace = TurnTrace::new("turn-1", Some("session-1".into()));
        let input = format!("{}\n{{torn", trace.to_jsonl().unwrap());
        let (parsed, skipped) = parse_jsonl(&input);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].turn_id, "turn-1");
        assert_eq!(skipped, 1);
    }
}
