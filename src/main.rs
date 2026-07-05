mod ide;
mod mcp;
mod tools;

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(name = "xmcp", version, about = "MCP server for controlling the Xojo IDE")]
struct Cli {
    /// Enable verbose logging to stderr
    #[arg(short, long)]
    verbose: bool,

    /// Path to Xojo documentation directory (auto-detected if omitted)
    #[arg(short, long)]
    docs_path: Option<PathBuf>,

    /// Read-only mode: hide and reject all tools that modify the project
    /// (set_code, set_selected_text, create_project_item, revert_project,
    /// save_project). Can also be enabled by setting XMCP_READ_ONLY=1.
    #[arg(long)]
    read_only: bool,
}

fn main() {
    let cli = Cli::parse();

    let read_only = cli.read_only || env_flag("XMCP_READ_ONLY");

    let docs_path = cli.docs_path.or_else(detect_docs_path);

    let ide = ide::communicator::Communicator::new(cli.verbose);

    if cli.verbose {
        if let Some(ref dp) = docs_path {
            eprintln!("Documentation path: {}", dp.display());
        } else {
            eprintln!("WARNING: Xojo documentation not found. Doc tools will be unavailable.");
        }
        eprintln!("IDE communicator initialized.");
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let all_tools = tools::all_tools();

    if cli.verbose {
        let active = all_tools.iter().filter(|t| !(read_only && t.mutates())).count();
        if read_only {
            eprintln!(
                "xmcp server configured with {active} tools ({} mutating tools disabled by read-only mode).",
                all_tools.len() - active
            );
        } else {
            eprintln!("xmcp server configured with {active} tools.");
        }
    }

    let server =
        mcp::server::Server::new(all_tools, Some(ide), docs_path, exe_dir, cli.verbose, read_only);
    server.run();
}

/// Returns true if the named environment variable is set to a truthy value
/// (`1`, `true`, `yes`, `on`, case-insensitive). Unset or empty is false.
fn env_flag(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

/// Auto-detect Xojo documentation path at
/// ~/Library/Application Support/Xojo/Xojo/<version>/Documentation/
fn detect_docs_path() -> Option<PathBuf> {
    let home = dirs_home()?;
    let xojo_dir = home.join("Library/Application Support/Xojo/Xojo");

    if !xojo_dir.is_dir() {
        return None;
    }

    let mut best_dir: Option<PathBuf> = None;
    let mut best_name = String::new();

    let entries = std::fs::read_dir(&xojo_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let docs_dir = path.join("Documentation");
        if !docs_dir.join("llms-full.txt").exists() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().into_owned();
        if best_dir.is_none() || compare_version_names(&name, &best_name) > 0 {
            best_name = name;
            best_dir = Some(docs_dir);
        }
    }

    best_dir
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Compare version directory names like "2024r9" vs "2024r10".
/// Extracts numeric parts and compares them segment by segment.
fn compare_version_names(a: &str, b: &str) -> i32 {
    let a_parts = extract_version_parts(a);
    let b_parts = extract_version_parts(b);

    if !a_parts.is_empty() || !b_parts.is_empty() {
        let max_count = a_parts.len().max(b_parts.len());
        for i in 0..max_count {
            let av = a_parts.get(i).copied().unwrap_or(0);
            let bv = b_parts.get(i).copied().unwrap_or(0);
            if av > bv {
                return 1;
            }
            if av < bv {
                return -1;
            }
        }
        return 0;
    }

    // Fallback to lexicographic.
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    match a_lower.cmp(&b_lower) {
        std::cmp::Ordering::Greater => 1,
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
    }
}

fn extract_version_parts(value: &str) -> Vec<i32> {
    let mut parts = Vec::new();
    let mut current = String::new();

    for ch in value.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            if let Ok(n) = current.parse() {
                parts.push(n);
            }
            current.clear();
        }
    }
    if !current.is_empty()
        && let Ok(n) = current.parse()
    {
        parts.push(n);
    }

    parts
}
