use serde_json::Value;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::reply;

const DEFAULT_SOCKET_NAME: &str = "XojoIDE";
/// Seven attempts back off for 6.3 s in total (see `retry_pause`). Xojo 2026r3
/// drops and recreates its socket after each connection and can stay away for
/// ~3 s, which outlasted the 1.5 s that five attempts gave.
const MAX_RETRIES: u32 = 7;

/// After the end marker only a compiler warning can follow, and it comes
/// within a millisecond (measured by upstream XMCP on 2026r2.1).
const AFTER_END_MARKER: Duration = Duration::from_millis(50);
/// Where no marker was appended, how long to keep reading after the first part.
const SPLIT_REPLY_WINDOW: Duration = Duration::from_millis(250);
/// How long a part already half read may take to finish once the answer ended.
const PARTIAL_PART: Duration = Duration::from_millis(1000);
/// How quiet a parked connection must be after its answer ended before it is
/// closed, so a trailing warning is in first.
const QUIET_AFTER_ANSWER: Duration = Duration::from_millis(250);
/// How long a parked connection is held before it is closed regardless.
const PENDING_GIVE_UP: Duration = Duration::from_secs(2 * 60 * 60);

/// Said with every message about a request the IDE still owes an answer to.
const DO_NOT_QUIT: &str = "Do not quit or restart Claude Code (or whichever MCP client you \
    use) until then: quitting closes this connection, and on macOS the Xojo IDE crashes if \
    it answers over a connection that has been closed.";

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

/// Parse every whole NUL-terminated frame from `cursor` on, advancing it past
/// them, and hand each readable one to `f`. An unreadable frame is skipped,
/// never fatal: the caller is reading from an IDE that may still be writing.
fn take_frames(buffer: &[u8], cursor: &mut usize, verbose: bool, mut f: impl FnMut(Value)) {
    while let Some(rel_pos) = buffer[*cursor..].iter().position(|&b| b == 0) {
        let frame_end = *cursor + rel_pos;
        let frame = std::str::from_utf8(&buffer[*cursor..frame_end])
            .ok()
            .map(str::trim)
            .and_then(|s| (!s.is_empty()).then(|| serde_json::from_str::<Value>(s)));
        *cursor = frame_end + 1;
        match frame {
            None => {}
            Some(Ok(v)) => {
                if verbose {
                    eprintln!("IDE response frame: {v}");
                }
                f(v);
            }
            Some(Err(e)) if verbose => eprintln!("Skipped an unreadable IPC frame: {e}"),
            Some(Err(_)) => {}
        }
    }
}

/// A request the IDE accepted but had not finished answering when its caller
/// gave up, and the connection it arrived on.
///
/// The connection is held open rather than closed. The IDE runs scripts one at
/// a time on its main thread, so a build or a modal dialog can leave a request
/// unanswered for minutes; when it does answer, a write into a closed socket
/// raises SIGPIPE, which the Xojo IDE does not ignore -- it dies with no crash
/// report (traced by upstream XMCP). `drain_pending` releases the connection
/// once the IDE is finished with it.
struct Pending {
    stream: UnixStream,
    tag: String,
    /// The script as the caller wrote it, for naming the request when refusing.
    script: String,
    marker_expected: bool,
    /// Bytes read but not yet framed; a part can only be read whole.
    buffer: Vec<u8>,
    since: Instant,
    last_data: Instant,
    answer_ended: bool,
}

/// Why a parked request was let go, or `None` while the IDE still owes it.
enum Release {
    Answered,
    Closed,
    GaveUp,
}

impl Pending {
    /// Read what the IDE has written so far and decide whether the answer is
    /// complete, by the live reader's rule: its end marker, or an error that
    /// stopped the script (or, with no marker coming, any part that is not
    /// warnings alone) -- and then a quiet spell, so a trailing warning is in
    /// before the close. What the parts say is discarded; their caller gave
    /// up long ago.
    fn poll(&mut self, verbose: bool) -> Option<Release> {
        if self.stream.set_nonblocking(true).is_err() {
            return Some(Release::Closed);
        }
        let mut read_buf = [0u8; 4096];
        loop {
            match self.stream.read(&mut read_buf) {
                Ok(0) => return Some(Release::Closed),
                Ok(n) => {
                    self.buffer.extend_from_slice(&read_buf[..n]);
                    self.last_data = Instant::now();
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Some(Release::Closed),
            }
        }

        let mut cursor = 0;
        let (tag, marker_expected) = (self.tag.clone(), self.marker_expected);
        let mut ended = self.answer_ended;
        take_frames(&self.buffer, &mut cursor, verbose, |response| {
            if response.get("tag").and_then(Value::as_str) != Some(tag.as_str()) {
                return;
            }
            if marker_expected {
                ended |= reply::is_end_marker(&response, &tag) || reply::stops_script(&response);
            } else {
                ended |= reply::kind(&response) != reply::Kind::Warning;
            }
        });
        self.buffer.drain(..cursor);
        self.answer_ended = ended;

        if self.answer_ended {
            // Our server only wakes on requests, so rather than turn the next
            // one away over a fraction of a second, wait out the quiet spell.
            let quiet = self.last_data.elapsed();
            if quiet < QUIET_AFTER_ANSWER {
                std::thread::sleep(QUIET_AFTER_ANSWER - quiet);
                return self.poll(verbose);
            }
            return Some(Release::Answered);
        }
        // The one release that closes a connection the IDE may still answer:
        // without a bound, a request it never answers would hold its only IPC
        // connection for as long as xmcp lives.
        if self.since.elapsed() > PENDING_GIVE_UP {
            return Some(Release::GaveUp);
        }
        None
    }
}

pub struct Communicator {
    tag_counter: AtomicU64,
    verbose: bool,
    pending: Mutex<Vec<Pending>>,
}

impl Communicator {
    pub fn new(verbose: bool) -> Self {
        Self {
            tag_counter: AtomicU64::new(0),
            verbose,
            pending: Mutex::new(Vec::new()),
        }
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, Vec<Pending>> {
        // A panic while the lock was held leaves the list itself intact.
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Release every parked request the IDE has finished with, and return a
    /// summary of those still waiting, one line each, or `None` if none are.
    fn drain_pending(&self) -> Option<String> {
        let mut pending = self.pending();
        pending.retain_mut(|p| match p.poll(self.verbose) {
            None => true,
            Some(why) => {
                if self.verbose {
                    let reason = match why {
                        Release::Answered => "the IDE answered it (reply discarded)",
                        Release::Closed => "the IDE closed the connection",
                        Release::GaveUp => "it was parked for over two hours",
                    };
                    eprintln!("IDE request {}: released, {reason}.", p.tag);
                }
                false
            }
        });
        if pending.is_empty() {
            return None;
        }
        Some(
            pending
                .iter()
                .map(|p| {
                    let preview: String = p.script.split_whitespace().collect::<Vec<_>>().join(" ");
                    let preview = match preview.char_indices().nth(77) {
                        Some((i, _)) => format!("{}...", &preview[..i]),
                        None => preview,
                    };
                    format!("  {} (sent {}s ago): {preview}", p.tag, p.since.elapsed().as_secs())
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// Keep a delivered request's connection open until the IDE has answered.
    fn park(&self, stream: UnixStream, tag: &str, script: &str, marker_expected: bool, unframed: &[u8]) {
        let now = Instant::now();
        self.pending().push(Pending {
            stream,
            tag: tag.to_string(),
            script: script.to_string(),
            marker_expected,
            buffer: unframed.to_vec(),
            since: now,
            last_data: now,
            answer_ended: false,
        });
    }

    /// Send an IDE script and receive the response with a custom timeout.
    pub fn send_and_receive_with_timeout(
        &self,
        script: &str,
        timeout: Duration,
    ) -> Result<Value, String> {
        // While the IDE still owes an answer to an earlier request it is busy
        // running that script; a new request would only queue behind it and
        // time out too. Say so instead.
        if let Some(summary) = self.drain_pending() {
            return Err(format!(
                "The Xojo IDE is still busy with an earlier request and has not finished \
                 answering it:\n{summary}\nNo new request was sent. Something is keeping the \
                 IDE busy -- usually a build, or a dialog in the IDE waiting for a click. Let it \
                 finish or click the dialog, then try again. {DO_NOT_QUIT}"
            ));
        }

        let tag = self.next_tag();

        // Every request must be answerable: the IDE replies once per Print and
        // not at all without one. The appended marker also tells us when the
        // answer is complete -- see reply.rs.
        let sent = reply::with_end_marker(script, &tag);
        let marker_expected = sent.len() != script.len();

        // Build protocol v2 payload: handshake + request, NUL-terminated.
        let proto = serde_json::json!({"protocol": 2});
        let request = serde_json::json!({"tag": tag, "script": sent});
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

                match self.try_send_receive(path, &payload, &tag, script, timeout, marker_expected) {
                    Ok(response) => {
                        return Ok(response);
                    }
                    Err(e) if e.script_sent => {
                        // Something accepted the script. Moving to the next
                        // candidate would hand the same command to a second
                        // IDE; retrying would hand it to this one twice. What
                        // the earlier attempts said -- usually "not found" --
                        // is beside the point now, and would bury the message.
                        return Err(e.message);
                    }
                    Err(e) => {
                        last_error_retryable = e.is_retryable();
                        all_errors.push(e.message);
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
        script: &str,
        timeout: Duration,
        marker_expected: bool,
    ) -> Result<Value, AttemptError> {
        let mut stream = UnixStream::connect(path).map_err(|e| {
            AttemptError::before_send(format!("IPCSocket connect failed at {path}: {e}"))
        })?;

        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .ok();

        // A write that fails partway may still have delivered the request, so
        // it is parked like any other delivered request rather than dropped:
        // if the connection is really broken, the next drain finds it closed
        // and releases it.
        if let Err(e) = stream.write_all(payload).and_then(|()| stream.flush()) {
            self.park(stream, tag, script, marker_expected, &[]);
            return Err(AttemptError::after_send(format!(
                "Writing the request to the Xojo IDE failed partway ({e}). It may or may not \
                 have arrived, so it is not being resent, and the connection is kept open in \
                 case the IDE answers. Further requests are refused until it does or the \
                 connection is found closed. {DO_NOT_QUIT}"
            )));
        }

        // One answer arrives as several parts under our tag -- one per Print,
        // then any compiler warning -- each sent the moment it is produced, so a
        // gap between parts says nothing about whether the answer is finished.
        // It is finished when the end marker arrives (plus a moment for a
        // trailing warning), or when an error stops the script before the
        // marker. Closing earlier is what this loop exists to prevent: the IDE
        // writes the rest into a closed socket and dies of SIGPIPE.
        let deadline = Instant::now() + timeout;
        let mut collect_until = deadline;
        let mut buffer: Vec<u8> = Vec::with_capacity(8192);
        let mut cursor: usize = 0;
        let mut read_buf = [0u8; 4096];
        let mut frames: Vec<Value> = Vec::new();
        let mut saw_marker = false;
        let mut saw_stop = false;
        let mut window_started = false;
        let mut ide_closed = false;

        loop {
            // Once the answer has ended, the short wait after it may run past
            // the deadline, and a part still half read is finished, briefly:
            // cutting either short would close on the IDE mid-write.
            let now = Instant::now();
            let ended = saw_marker || window_started;
            let half_read = buffer[cursor..].iter().any(|b| !b.is_ascii_whitespace());
            if now >= collect_until && !(ended && half_read && now < collect_until + PARTIAL_PART) {
                break;
            }

            match stream.read(&mut read_buf) {
                Ok(0) => {
                    ide_closed = true;
                    break;
                }
                Ok(n) => {
                    buffer.extend_from_slice(&read_buf[..n]);
                }
                Err(ref e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    continue;
                }
                Err(e) => {
                    // Stop reading; what has arrived decides below whether the
                    // answer is complete or the request parks. Returning here
                    // would drop a connection the IDE may still answer on.
                    if self.verbose {
                        eprintln!("IDE request {tag}: reading failed ({e}).");
                    }
                    break;
                }
            }

            take_frames(&buffer, &mut cursor, self.verbose, |response| {
                if response.get("tag").and_then(Value::as_str) != Some(tag) {
                    // Not our tag — a stale answer for another request.
                    return;
                }
                if reply::is_end_marker(&response, tag) {
                    saw_marker = true;
                    collect_until = Instant::now() + AFTER_END_MARKER;
                    return;
                }
                saw_stop |= reply::stops_script(&response);
                let kind = reply::kind(&response);
                frames.push(response);
                // Where no marker is coming, a short window after the first
                // part that is not warnings alone ends the wait.
                if !saw_marker
                    && !window_started
                    && (saw_stop || (!marker_expected && kind != reply::Kind::Warning))
                {
                    window_started = true;
                    collect_until = Instant::now() + SPLIT_REPLY_WINDOW;
                }
            });

            // Periodic compaction: reclaim consumed prefix once it dominates
            // the buffer. Bounded total shift cost — each byte moves at most
            // once between compactions, keeping the loop O(n) amortized.
            if cursor > 4096 && cursor * 2 > buffer.len() {
                buffer.drain(..cursor);
                cursor = 0;
            }
        }

        // The marker arrived: the script ran to its last line. A script that
        // printed nothing gets the reply the IDE sends for Print "".
        if saw_marker {
            return Ok(reply::merge(frames).unwrap_or_else(|| reply::empty(tag)));
        }
        // An error stopped the script, so nothing more is coming.
        if saw_stop {
            return Ok(reply::merge(frames).unwrap_or_else(|| reply::empty(tag)));
        }
        // No marker was appended; a window after a part that is not warnings
        // alone is the only end there is.
        if !marker_expected
            && !ide_closed
            && frames.iter().any(|f| reply::kind(f) != reply::Kind::Warning)
            && let Some(merged) = reply::merge(frames)
        {
            return Ok(merged);
        }
        if ide_closed {
            return Err(AttemptError::after_send(format!(
                "The Xojo IDE at {path} closed the connection before it finished answering. \
                 It may have quit or crashed, or run the script and then closed it, so the \
                 request is not being sent again."
            )));
        }
        // Accepted, not finished: the IDE is still running the script, even if
        // part of its answer is in. Do not close the connection -- the IDE will
        // write the rest when it is done. Park it, with whatever was read but
        // not yet framed: the limit can fall mid-part, and if that part is the
        // marker, dropping its first half would leave the parked request unable
        // to see its own end.
        self.park(stream, tag, script, marker_expected, &buffer[cursor..]);
        Err(AttemptError::after_send(format!(
            "The Xojo IDE accepted the request but has not finished answering it within {} ms. \
             It is most likely busy -- a build, or a dialog waiting for a click -- and it will \
             finish the request regardless, so it is not being sent again. The connection is \
             kept open so the IDE can answer safely; that answer will be discarded. Further \
             requests are refused until the IDE has answered. {DO_NOT_QUIT}",
            timeout.as_millis()
        )))
    }

    fn next_tag(&self) -> String {
        let n = self.tag_counter.fetch_add(1, Ordering::Relaxed);
        format!("xmcp_{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AttemptError, Communicator, DEFAULT_SOCKET_NAME, Release, candidate_paths, resolve_socket_name,
        summarize_errors,
    };
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

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

    /// A parked request on one end of a socket pair; the other end plays the IDE.
    fn parked(script: &str) -> (Communicator, UnixStream) {
        let (ours, ide) = UnixStream::pair().unwrap();
        let c = Communicator::new(false);
        c.park(ours, "xmcp_7", script, true, &[]);
        (c, ide)
    }

    fn frame(ide: &mut UnixStream, json: &str) {
        ide.write_all(json.as_bytes()).unwrap();
        ide.write_all(&[0]).unwrap();
    }

    fn poll_first(c: &Communicator) -> Option<Release> {
        c.pending()[0].poll(false)
    }

    #[test]
    fn a_parked_request_blocks_new_ones_and_names_itself() {
        let (c, _ide) = parked("DoCommand \"BuildApp\"");
        let err = c
            .send_and_receive_with_timeout("Print 1", Duration::from_millis(100))
            .unwrap_err();
        assert!(err.contains("still busy"), "{err}");
        assert!(err.contains("xmcp_7 (sent 0s ago): DoCommand \"BuildApp\""), "{err}");
        assert!(err.contains("Do not quit"), "{err}");
        assert_eq!(c.pending().len(), 1, "refusing must not release it");
    }

    #[test]
    fn output_alone_does_not_end_a_parked_answer() {
        let (c, mut ide) = parked("Print 1");
        frame(&mut ide, r#"{"tag":"xmcp_7","response":"1"}"#);
        assert!(poll_first(&c).is_none(), "the marker is still to come");
    }

    #[test]
    fn the_marker_releases_a_parked_request_after_a_quiet_spell() {
        let (c, mut ide) = parked("Print 1");
        frame(&mut ide, r#"{"tag":"other","response":"xmcp-end:xmcp_7"}"#);
        assert!(poll_first(&c).is_none(), "another tag says nothing about ours");
        // The marker arrives in two pieces; only the whole part counts.
        ide.write_all(br#"{"tag":"xmcp_7","respo"#).unwrap();
        assert!(poll_first(&c).is_none());
        ide.write_all(br#"nse":"xmcp-end:xmcp_7"}"#).unwrap();
        ide.write_all(&[0]).unwrap();
        let start = Instant::now();
        assert!(matches!(poll_first(&c), Some(Release::Answered)));
        assert!(start.elapsed() >= Duration::from_millis(200), "waited for a trailing warning");
        assert!(c.drain_pending().is_none(), "released");
        assert!(c.pending().is_empty());
    }

    #[test]
    fn an_error_that_stops_the_script_ends_a_parked_answer() {
        let (c, mut ide) = parked("Print x");
        frame(&mut ide, r#"{"tag":"xmcp_7","response":{"scriptError":[{"type":"scriptRuntimeError"}]}}"#);
        assert!(matches!(poll_first(&c), Some(Release::Answered)));
    }

    #[test]
    fn a_closed_connection_releases_a_parked_request() {
        let (c, ide) = parked("Print 1");
        drop(ide);
        assert!(matches!(poll_first(&c), Some(Release::Closed)));
        assert!(c.drain_pending().is_none());
    }
}
