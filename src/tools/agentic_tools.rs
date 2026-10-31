use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::mcp::tool::*;
use crate::tools::*;

/// Dropped into the project folder to ask an Agentic Mode IDE for a run.
const RUN_TRIGGER: &str = "XojoRunProject.txt";
/// Written beside it when that run fails to compile, one error per line.
const COMPILE_ERRORS: &str = "XojoCompileErrors.txt";

/// How long Xojo gets to notice the trigger. 2026r3 took 1.1–2.1 s on a small
/// project; a trigger still there after this means nothing is watching.
const PICKUP_BUDGET: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

/// Whether the saved project file has Agentic Mode on. The IDE writes
/// `AgenticMode=True` into the `.xojo_project` manifest, so this reflects the
/// last save, not a toggle made since.
pub fn agentic_mode_saved(manifest: &str) -> bool {
    manifest
        .lines()
        .any(|l| l.trim().eq_ignore_ascii_case("AgenticMode=True"))
}

/// Turn the raw contents of `XojoCompileErrors.txt` into a tool message. Each
/// line reads `<location>, line <n>: <message>`; the message can itself
/// contain colons, so lines are passed through whole rather than split.
pub fn format_compile_errors(raw: &str) -> String {
    let errors: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let noun = if errors.len() == 1 { "error" } else { "errors" };
    let mut msg = format!("Compile failed with {} {noun}:\n", errors.len());
    for e in &errors {
        msg.push_str("  ");
        msg.push_str(e);
        msg.push('\n');
    }
    msg.push_str(
        "Line numbers count from the first line of the method body. \
         Fix the errors and run again.",
    );
    msg
}

/// PIDs of debug builds launched from `dir`. Xojo places the debug app in the
/// project folder as `<Name>.debug.app`, so its executable path starts there.
/// `None` when `pgrep` is unavailable, in which case success is not detected
/// early and the caller falls back to waiting out its budget.
fn debug_app_pids(dir: &Path) -> Option<HashSet<u32>> {
    let pattern = format!("{}/.*\\.debug", dir.display());
    let out = Command::new("pgrep").args(["-f", &pattern]).output().ok()?;
    // pgrep exits 1 when nothing matches, which is an answer, not a failure.
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect(),
    )
}

/// Read the error file once it has stopped growing, so a run caught mid-write
/// does not report half its errors.
fn read_settled(path: &Path) -> String {
    let mut last = String::new();
    for _ in 0..10 {
        let now = std::fs::read_to_string(path).unwrap_or_default();
        if !now.is_empty() && now == last {
            break;
        }
        last = now;
        std::thread::sleep(Duration::from_millis(200));
    }
    last
}

/// What a run produced.
#[derive(Debug, PartialEq)]
enum RunOutcome {
    /// Xojo never took the trigger; it has been removed again.
    NotPickedUp,
    Errors(String),
    /// A debug app that was not running before the trigger appeared.
    Launched,
    /// Neither errors nor a new app within the budget.
    Quiet,
}

/// Drive one Agentic Mode run in `dir`: give Xojo `pickup` to take the trigger,
/// then wait up to `wait` for its result.
/// `pids` is how running debug apps are found, injectable so the waiting logic
/// can be tested without Xojo.
fn agentic_run(
    dir: &Path,
    pickup: Duration,
    wait: Duration,
    pids: &dyn Fn(&Path) -> Option<HashSet<u32>>,
) -> std::io::Result<RunOutcome> {
    let trigger = dir.join(RUN_TRIGGER);
    let errors = dir.join(COMPILE_ERRORS);

    // A leftover from an earlier run would read as this run's result. Xojo
    // clears it on a clean run, but only once it gets that far.
    match std::fs::remove_file(&errors) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let before = pids(dir);
    std::fs::write(&trigger, b"")?;

    let deadline = Instant::now() + pickup;
    while trigger.exists() {
        if Instant::now() >= deadline {
            // Left in place, a trigger would fire whenever the mode is next
            // switched on -- an app launch nobody asked for by then.
            let _ = std::fs::remove_file(&trigger);
            return Ok(RunOutcome::NotPickedUp);
        }
        std::thread::sleep(POLL);
    }

    let deadline = Instant::now() + wait;
    let mut next_pid_check = Instant::now();
    loop {
        if errors.exists() {
            return Ok(RunOutcome::Errors(read_settled(&errors)));
        }
        // pgrep is a process spawn, so ask it less often than the file system.
        if Instant::now() >= next_pid_check {
            next_pid_check = Instant::now() + Duration::from_millis(500);
            if let (Some(before), Some(now)) = (&before, pids(dir))
                && now.difference(before).next().is_some()
            {
                return Ok(RunOutcome::Launched);
            }
        }
        if Instant::now() >= deadline {
            return Ok(RunOutcome::Quiet);
        }
        std::thread::sleep(POLL);
    }
}

/// The project directory: the argument when given, else the folder of the
/// project the IDE has open.
fn resolve_project_dir(
    args: &HashMap<String, Value>,
    ctx: &ToolContext,
) -> Result<PathBuf, ToolResult> {
    let given = arg_str(args, "project_dir", "");
    let dir = if !given.trim().is_empty() {
        PathBuf::from(given.trim())
    } else {
        let r = ide_call_default(ctx, "Print ProjectShellPath");
        if r.is_error {
            return Err(ToolResult::failure(format!(
                "{} — pass project_dir to run without the IDE socket.",
                r.output
            )));
        }
        let path = r.output.trim();
        if path.is_empty() {
            return Err(ToolResult::failure(
                "ProjectShellPath was empty — is a project open in the Xojo IDE?",
            ));
        }
        match Path::new(path).parent() {
            Some(p) => p.to_path_buf(),
            None => {
                return Err(ToolResult::failure(format!(
                    "No folder for project path '{path}'."
                )));
            }
        }
    };
    if !dir.is_dir() {
        return Err(ToolResult::failure(format!(
            "'{}' is not a directory.",
            dir.display()
        )));
    }
    Ok(dir)
}

// ---------------------------------------------------------------------------
// agentic_run
// ---------------------------------------------------------------------------
pub struct AgenticRun;

impl Tool for AgenticRun {
    fn name(&self) -> &'static str {
        "agentic_run"
    }
    fn annotations(&self) -> ToolAnnotations {
        ToolAnnotations::writes("Run in Agentic Mode", false, false)
    }
    fn description(&self) -> &'static str {
        "Agentic Mode only (Xojo 2026r3+): runs the project and returns its compile \
         errors, if any. Unlike run_project, this reports real compiler errors with \
         their locations. It works through files in the project folder rather than \
         IDE scripting, so it also works when the IDE socket does not: it drops \
         XojoRunProject.txt into the project folder and reads back XojoCompileErrors.txt. \
         Compile errors fail the call, one '<location>, line <n>: <message>' per line. A \
         successful run launches the app in the debugger; stop it with stop_project. If Xojo \
         has not taken the request within 10 seconds, Agentic Mode is off or the project is \
         not open: the request file is removed again and the call fails saying how to turn \
         the mode on. Agentic Mode is the user's opt-in to having the project run to check \
         edits, so this may be called without asking each time."
    }
    fn parameters(&self) -> &[ToolParam] {
        static P: &[ToolParam] = &[
            ToolParam {
                name: "project_dir",
                param_type: ParamType::String,
                description: "Absolute path of the folder containing the .xojo_project. Omit to \
                          use the project open in the IDE, which needs the IDE socket.",
                required: false,
                default: None,
            },
            ToolParam {
                name: "wait_seconds",
                param_type: ParamType::Integer,
                description: "How long to wait for errors or a launch once Xojo starts the run (default 15, max 120). Raise it for large projects.",
                required: false,
                default: None,
            },
        ];
        P
    }
    fn run(&self, args: &HashMap<String, Value>, ctx: &ToolContext) -> ToolResult {
        let dir = match resolve_project_dir(args, ctx) {
            Ok(d) => d,
            Err(r) => return r,
        };
        let wait = Duration::from_secs(arg_i64(args, "wait_seconds", 15).clamp(1, 120) as u64);

        match agentic_run(&dir, PICKUP_BUDGET, wait, &debug_app_pids) {
            Err(e) => {
                ToolResult::failure(format!("Could not drive a run in '{}': {e}", dir.display()))
            }
            Ok(RunOutcome::NotPickedUp) => ToolResult::failure(format!(
                "Xojo did not pick up {RUN_TRIGGER} within {} s, so Agentic Mode is not \
                 on for this project, the project is not open, or the IDE is busy. \
                 Turn it on with File > Agentic Mode (Xojo 2026r3+), or use run_project.",
                PICKUP_BUDGET.as_secs()
            )),
            Ok(RunOutcome::Errors(raw)) => ToolResult::failure(format_compile_errors(&raw)),
            Ok(RunOutcome::Launched) => {
                ToolResult::success("Compiled cleanly; the app is running in the debugger.")
            }
            Ok(RunOutcome::Quiet) => ToolResult::success(format!(
                "No compile errors reported within {} s. Xojo writes {COMPILE_ERRORS} only \
                 when compilation fails, so this usually means the build succeeded, but the \
                 debug app was not seen starting. If the project is large, run again with a \
                 higher wait_seconds.",
                wait.as_secs()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch_dir() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "xmcp-agentic-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Stand in for Xojo: take the trigger after `delay`, then run `then`.
    fn fake_ide(dir: &Path, delay: Duration, then: impl FnOnce(&Path) + Send + 'static) {
        let dir = dir.to_path_buf();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            std::fs::remove_file(dir.join(RUN_TRIGGER)).unwrap();
            then(&dir);
        });
    }

    fn no_pids(_: &Path) -> Option<HashSet<u32>> {
        Some(HashSet::new())
    }

    #[test]
    fn saved_flag_is_read_from_the_manifest() {
        assert!(agentic_mode_saved(
            "Type=Desktop\nAgenticMode=True\nMDI=0\n"
        ));
        assert!(!agentic_mode_saved("Type=Desktop\nAgenticMode=False\n"));
        assert!(!agentic_mode_saved("Type=Desktop\nMDI=0\n"));
    }

    #[test]
    fn errors_are_counted_and_passed_through_whole() {
        let raw = "Window1.Opening, line 1: Type mismatch error.  Expected Integer, but got TextLiteral: Dim x As Integer = \"a\"\n\
                   Window1.Opening, line 2: This item does not exist: UndefinedThing";
        let msg = format_compile_errors(raw);
        assert!(msg.starts_with("Compile failed with 2 errors:\n"));
        assert!(
            msg.contains("  Window1.Opening, line 2: This item does not exist: UndefinedThing\n")
        );
        assert!(
            format_compile_errors("A, line 1: x").starts_with("Compile failed with 1 error:\n")
        );
    }

    #[test]
    fn compile_errors_are_returned() {
        let dir = scratch_dir();
        fake_ide(&dir, Duration::from_millis(300), |d| {
            std::fs::write(d.join(COMPILE_ERRORS), "App.Foo, line 1: bad").unwrap();
        });
        let out = agentic_run(
            &dir,
            Duration::from_secs(2),
            Duration::from_secs(5),
            &no_pids,
        )
        .unwrap();
        assert_eq!(out, RunOutcome::Errors("App.Foo, line 1: bad".into()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_stale_error_file_is_not_mistaken_for_this_run() {
        let dir = scratch_dir();
        std::fs::write(dir.join(COMPILE_ERRORS), "Old.Thing, line 9: stale").unwrap();
        fake_ide(&dir, Duration::from_millis(200), |_| {});
        let out = agentic_run(
            &dir,
            Duration::from_secs(2),
            Duration::from_secs(1),
            &no_pids,
        )
        .unwrap();
        assert_eq!(out, RunOutcome::Quiet);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unanswered_trigger_is_taken_back() {
        let dir = scratch_dir();
        let out = agentic_run(
            &dir,
            Duration::from_millis(300),
            Duration::from_secs(1),
            &no_pids,
        )
        .unwrap();
        assert_eq!(out, RunOutcome::NotPickedUp);
        assert!(!dir.join(RUN_TRIGGER).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn only_a_new_debug_app_counts_as_a_launch() {
        // An app already running before the trigger -- the restart case -- must
        // not end the wait; the PID that replaces it must.
        let dir = scratch_dir();
        let started = Instant::now();
        let pids = move |_: &Path| -> Option<HashSet<u32>> {
            if started.elapsed() < Duration::from_millis(800) {
                Some(HashSet::from([100]))
            } else {
                Some(HashSet::from([200]))
            }
        };
        fake_ide(&dir, Duration::from_millis(200), |_| {});
        let out = agentic_run(&dir, Duration::from_secs(2), Duration::from_secs(5), &pids).unwrap();
        assert_eq!(out, RunOutcome::Launched);
        assert!(started.elapsed() >= Duration::from_millis(800));
        std::fs::remove_dir_all(&dir).ok();
    }
}
