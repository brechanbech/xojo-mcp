use serde_json::Value;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const DEFAULT_SOCKET_NAME: &str = "XojoIDE";
/// Seven attempts back off for 6.3 s in total (see `retry_pause`). Xojo 2026r3
/// drops and recreates its socket after each connection and can stay away for
/// ~3 s, which outlasted the 1.5 s that five attempts gave.
const MAX_RETRIES: u32 = 7;

/// Resolve the IPC socket file name.
///
/// The IDE appends `XojoIDE` — or the value of `XOJO_IPCPATH`, when set — to a
/// temporary directory. `XOJO_IPCPATH` is how you talk to a specific instance
/// when several IDEs run at once; set the same value in xmcp's environment.
/// Xojo documents `a-z A-Z 0-9 _` as the only valid characters, so anything
/// else falls back to the default rather than probing a path the IDE would
/// never have created.
fn socket_name() -> String {
    resolve_socket_name(std::env::var("XOJO_IPCPATH").ok())
}

/// Pure form of [`socket_name`], split out so the validation rule is testable
/// without touching the process environment.
fn resolve_socket_name(raw: Option<String>) -> String {
    match raw {
        Some(name) if name.is_empty() => DEFAULT_SOCKET_NAME.to_string(),
        Some(name) => {
            if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                name
            } else {
                eprintln!(
                    "xmcp: ignoring XOJO_IPCPATH={name:?} (only a-z A-Z 0-9 _ are valid); \
                     using {DEFAULT_SOCKET_NAME}"
                );
                DEFAULT_SOCKET_NAME.to_string()
            }
        }
        None => DEFAULT_SOCKET_NAME.to_string(),
    }
}

/// Socket paths to try, in the IDE's own search order: `/tmp` first, then the
/// system temporary directory (`SpecialFolder.Temporary`, i.e. `TMPDIR`) that
/// the IDE falls back to when `/tmp` is not writable. `/private/tmp` is kept
/// as an explicit candidate because it is what `/tmp` resolves to on macOS.
fn socket_candidates() -> Vec<String> {
    candidate_paths(
        &socket_name(),
        std::env::var_os("TMPDIR"),
        std::env::var_os("HOME"),
    )
}

/// Pure form of [`socket_candidates`], split out for testing.
///
/// Xojo documents the socket path (2026r2.1, issue 68115) as the name appended
/// to the first *writable* temporary directory, and the `IDECommunicator`
/// example it ships probes a wider chain than the documentation describes:
/// `/tmp`, `/var/tmp`, `SpecialFolder.Temporary`, then the home folder. We probe
/// all of it. An IDE that settled on a rung we would not have picked is still
/// reachable, and a rung that holds no socket costs one `exists` check.
///
/// `/private/tmp` is here because `/tmp` is a symlink to it on macOS; both
/// resolve to the same socket and [`unique_socket_paths`] drops the duplicate.
fn candidate_paths(
    name: &str,
    tmpdir: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Vec<String> {
    let mut paths = vec![
        format!("/tmp/{name}"),
        format!("/private/tmp/{name}"),
        format!("/var/tmp/{name}"),
    ];

    for dir in [tmpdir, home].into_iter().flatten() {
        if dir.is_empty() {
            continue;
        }
        paths.push(
            std::path::Path::new(&dir)
                .join(name)
                .to_string_lossy()
                .into_owned(),
        );
    }

    paths
}

/// Deduplicate socket paths by canonical path.
/// On macOS, /tmp is a symlink to /private/tmp, so both candidates resolve
/// to the same socket. Without deduplication we waste a full timeout cycle
/// on what is effectively a second attempt at the same socket.
fn unique_socket_paths() -> Vec<String> {
    let mut seen = HashSet::new();
    socket_candidates()
        .into_iter()
        .filter(|p| {
            let canonical = std::fs::canonicalize(p)
                .map(|c| c.to_string_lossy().to_string())
                .unwrap_or_else(|_| p.clone());
            seen.insert(canonical)
        })
        .collect()
}

/// Why one attempt at one socket failed, and whether the script reached the IDE.
///
/// The flag is the whole reason this is a type rather than a string. A failure
/// *before* the payload is written can be retried safely, because nothing ran.
/// Once the bytes are on the wire the IDE is executing them, and silence
/// afterwards says only that we did not hear the answer back -- resending would
/// run the command a second time. For a read that is merely wasteful; for
/// `set_code`, `create_project_item` or a build it is a duplicated edit.
struct AttemptError {
    message: String,
    /// True from the moment writing *begins*, not once it completes. A partial
    /// write leaves an incomplete frame that the IDE will not execute, so
    /// erring this way costs at most one retry we could have had -- against the
    /// far worse failure of repeating a command that did run.
    script_sent: bool,
}

impl AttemptError {
    /// A failure that happened before anything was sent.
    fn before_send(message: String) -> Self {
        Self {
            message,
            script_sent: false,
        }
    }

    /// A failure after the script was handed to the socket.
    fn after_send(message: String) -> Self {
        Self {
            message,
            script_sent: true,
        }
    }

    /// Worth another attempt? Only when nothing ran *and* the cause looks
    /// transient -- in practice an IDE that has not finished starting up.
    fn is_retryable(&self) -> bool {
        !self.script_sent
            && (self.message.contains("not found") || self.message.contains("Connection refused"))
    }
}

/// Exponential backoff between retry attempts: 100, 200, 400, 800 ms ...
/// The shift is capped to keep the helper safe if MAX_RETRIES is ever raised.
fn retry_pause(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(6);
    Duration::from_millis(100u64 << shift)
}

/// Collapse repeated errors into "<err> (×N)" form while preserving the
/// order distinct errors first appeared. Keeps the final retry-failure
/// message readable when every attempt fails the same way.
fn summarize_errors(errors: &[String]) -> String {
    let mut summary: Vec<(String, usize)> = Vec::new();
    for e in errors {
        if let Some(entry) = summary.iter_mut().find(|(s, _)| s == e) {
            entry.1 += 1;
        } else {
            summary.push((e.clone(), 1));
        }
    }
    summary
        .into_iter()
        .map(|(s, n)| if n > 1 { format!("{s} (×{n})") } else { s })
        .collect::<Vec<_>>()
        .join("; ")
}

pub struct Communicator {
    tag_counter: AtomicU64,
    verbose: bool,
}

impl Communicator {
    pub fn new(verbose: bool) -> Self {
        Self {
            tag_counter: AtomicU64::new(0),
            verbose,
        }
    }

    /// Send an IDE script and receive the response with a custom timeout.
    pub fn send_and_receive_with_timeout(
        &self,
        script: &str,
        timeout: Duration,
    ) -> Result<Value, String> {
        let tag = self.next_tag();

        // Build protocol v2 payload: handshake + request, NUL-terminated.
        let proto = serde_json::json!({"protocol": 2});
        let request = serde_json::json!({"tag": tag, "script": script});
        let mut payload = Vec::new();
        payload.extend_from_slice(proto.to_string().as_bytes());
        payload.push(0); // NUL terminator
        payload.extend_from_slice(request.to_string().as_bytes());
        payload.push(0); // NUL terminator

        let candidates = unique_socket_paths();
        let mut all_errors = Vec::new();
        let mut last_error_retryable = true;

        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                if !last_error_retryable {
                    break;
                }
                std::thread::sleep(retry_pause(attempt));
            }

            for path in &candidates {
                if !std::path::Path::new(path).exists() {
                    let err = format!("IPC socket not found at: {path}");
                    all_errors.push(err);
                    // "not found" is retryable — IDE may not have started yet.
                    last_error_retryable = true;
                    continue;
                }

                match self.try_send_receive(path, &payload, &tag, timeout) {
                    Ok(response) => {
                        return Ok(response);
                    }
                    Err(e) => {
                        let sent = e.script_sent;
                        last_error_retryable = e.is_retryable();
                        all_errors.push(e.message);
                        if sent {
                            // Something accepted the script. Moving to the next
                            // candidate would hand the same command to a second
                            // IDE; retrying would hand it to this one twice.
                            break;
                        }
                    }
                }
            }
        }

        let msg = if all_errors.is_empty() {
            "No IPC socket candidates found.".to_string()
        } else {
            summarize_errors(&all_errors)
        };
        Err(msg)
    }

    fn try_send_receive(
        &self,
        path: &str,
        payload: &[u8],
        tag: &str,
        timeout: Duration,
    ) -> Result<Value, AttemptError> {
        let mut stream = UnixStream::connect(path).map_err(|e| {
            AttemptError::before_send(format!("IPCSocket connect failed at {path}: {e}"))
        })?;

        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .ok();

        stream
            .write_all(payload)
            .map_err(|e| AttemptError::after_send(format!("IPCSocket write failed: {e}")))?;
        stream
            .flush()
            .map_err(|e| AttemptError::after_send(format!("IPCSocket flush failed: {e}")))?;

        // Read response frames until we find one with our tag.
        let deadline = Instant::now() + timeout;
        let mut buffer: Vec<u8> = Vec::with_capacity(8192);
        let mut cursor: usize = 0;
        let mut read_buf = [0u8; 4096];

        loop {
            if Instant::now() >= deadline {
                return Err(AttemptError::after_send(format!(
                    "No IPCSocket response from {path} (timeout)"
                )));
            }

            match stream.read(&mut read_buf) {
                Ok(0) => {
                    return Err(AttemptError::after_send(format!(
                        "IPCSocket connection closed by {path}"
                    )));
                }
                Ok(n) => {
                    buffer.extend_from_slice(&read_buf[..n]);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    continue;
                }
                Err(e) => {
                    return Err(AttemptError::after_send(format!(
                        "IPCSocket read failed: {e}"
                    )));
                }
            }

            // Consume NUL-delimited frames in place, advancing a cursor rather
            // than reallocating the buffer per frame.
            while let Some(rel_pos) = buffer[cursor..].iter().position(|&b| b == 0) {
                let frame_end = cursor + rel_pos;
                let frame_str = std::str::from_utf8(&buffer[cursor..frame_end])
                    .map_err(|e| AttemptError::after_send(format!("Invalid UTF-8 in IPC frame: {e}")))?
                    .trim();
                cursor = frame_end + 1;

                if frame_str.is_empty() {
                    continue;
                }

                if self.verbose {
                    eprintln!("IDE response frame: {frame_str}");
                }

                let response: Value = serde_json::from_str(frame_str)
                    .map_err(|e| AttemptError::after_send(format!("Invalid JSON in IPC frame: {e}")))?;

                if response.get("tag").and_then(|t| t.as_str()) == Some(tag) {
                    return Ok(response);
                }
                // Not our tag — discard orphaned frame and keep reading.
            }

            // Periodic compaction: reclaim consumed prefix once it dominates
            // the buffer. Bounded total shift cost — each byte moves at most
            // once between compactions, keeping the loop O(n) amortized.
            if cursor > 4096 && cursor * 2 > buffer.len() {
                buffer.drain(..cursor);
                cursor = 0;
            }
        }
    }

    fn next_tag(&self) -> String {
        let n = self.tag_counter.fetch_add(1, Ordering::Relaxed);
        format!("xmcp_{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AttemptError, DEFAULT_SOCKET_NAME, candidate_paths, resolve_socket_name, summarize_errors,
    };

    #[test]
    fn a_script_already_sent_is_never_retried() {
        // The message deliberately looks transient. It must not matter: the IDE
        // has the script, so a resend would run the command twice.
        for message in [
            "No IPCSocket response from /tmp/XojoIDE (timeout)",
            "IPCSocket connection closed by /tmp/XojoIDE",
            "IPCSocket read failed: not found",
        ] {
            let err = AttemptError::after_send(message.to_owned());
            assert!(
                !err.is_retryable(),
                "post-send error was treated as retryable: {message}"
            );
        }
    }

    #[test]
    fn a_transient_failure_before_sending_is_retried() {
        // Nothing ran, and the IDE may still be starting up.
        assert!(
            AttemptError::before_send("IPC socket not found at: /tmp/XojoIDE".to_owned())
                .is_retryable()
        );
        assert!(
            AttemptError::before_send(
                "IPCSocket connect failed at /tmp/XojoIDE: Connection refused".to_owned()
            )
            .is_retryable()
        );
    }

    #[test]
    fn a_permanent_failure_before_sending_is_not_retried() {
        // A stale socket from a crashed IDE, or a permission problem: retrying
        // only delays an error that will not go away.
        assert!(
            !AttemptError::before_send(
                "IPCSocket connect failed at /tmp/XojoIDE: Permission denied".to_owned()
            )
            .is_retryable()
        );
    }

    #[test]
    fn collapses_identical_errors() {
        let errs = vec!["socket not found".to_string(); 5];
        assert_eq!(summarize_errors(&errs), "socket not found (×5)");
    }

    #[test]
    fn preserves_first_seen_order_with_counts() {
        let errs = vec![
            "a".to_string(),
            "b".to_string(),
            "a".to_string(),
            "b".to_string(),
            "b".to_string(),
        ];
        assert_eq!(summarize_errors(&errs), "a (×2); b (×3)");
    }

    #[test]
    fn singletons_have_no_count_suffix() {
        let errs = vec!["only one".to_string()];
        assert_eq!(summarize_errors(&errs), "only one");
    }

    #[test]
    fn empty_input_returns_empty_string() {
        let errs: Vec<String> = Vec::new();
        assert_eq!(summarize_errors(&errs), "");
    }

    #[test]
    fn socket_name_defaults_without_xojo_ipcpath() {
        assert_eq!(resolve_socket_name(None), DEFAULT_SOCKET_NAME);
        assert_eq!(
            resolve_socket_name(Some(String::new())),
            DEFAULT_SOCKET_NAME
        );
    }

    #[test]
    fn socket_name_uses_valid_xojo_ipcpath() {
        assert_eq!(
            resolve_socket_name(Some("Xojo2026r2_1".into())),
            "Xojo2026r2_1"
        );
    }

    #[test]
    fn socket_name_rejects_invalid_characters() {
        // Xojo documents a-z A-Z 0-9 _ as the only valid characters, so a value
        // with a separator or dot could never match a socket the IDE created.
        for bad in ["../escape", "Xojo 2026", "Xojo2026r2.1"] {
            assert_eq!(resolve_socket_name(Some(bad.into())), DEFAULT_SOCKET_NAME);
        }
    }

    #[test]
    fn candidates_cover_the_documented_chain_in_order() {
        let paths = candidate_paths(
            "XojoIDE",
            Some("/var/folders/xx/T/".into()),
            Some("/Users/someone".into()),
        );
        assert_eq!(
            paths,
            vec![
                "/tmp/XojoIDE".to_string(),
                "/private/tmp/XojoIDE".to_string(),
                "/var/tmp/XojoIDE".to_string(),
                "/var/folders/xx/T/XojoIDE".to_string(),
                "/Users/someone/XojoIDE".to_string(),
            ]
        );
    }

    #[test]
    fn candidates_skip_unset_and_empty_env() {
        // The three fixed rungs remain; an unset or empty variable adds nothing
        // rather than contributing a path rooted at the working directory.
        assert_eq!(candidate_paths("XojoIDE", None, None).len(), 3);
        assert_eq!(
            candidate_paths("XojoIDE", Some("".into()), Some("".into())).len(),
            3
        );
    }
}
