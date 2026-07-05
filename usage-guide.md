# xmcp Usage Guide for AI Assistants

This file is automatically loaded as an MCP resource when you connect to xmcp. It describes xmcp's capabilities, known limitations, and how to choose the right approach for each task. You can edit this file to add project-specific notes or customise the guidance.

---

## Prerequisites — before using any xmcp tools

**xmcp cannot start Xojo IDE.** All tools communicate via a macOS domain socket (`/tmp/XojoIDE`) that Xojo IDE creates when it launches. If the IDE is not running, every tool call will fail with "IPC socket not found".

**The user must:**

1. Start Xojo IDE manually
2. Open the project they want to work with (File > Open) — xmcp cannot open projects
3. Wait a few seconds after launch before the IPC socket is ready — if tools fail immediately after IDE start, ask the user to wait and retry

**Do not attempt any xmcp tool calls until the user confirms that Xojo IDE is open and the project is loaded.**

---

## Read-only mode

xmcp may be running in **read-only mode** (started with `--read-only` or
`XMCP_READ_ONLY=1`). When it is, the tools that modify the project —
`set_code`, `edit_code`, `set_selected_text`, `create_project_item`,
`revert_project`, and `save_project` — are not listed and cannot be called. If you don't see those
tools, the user has intentionally opened the project for browsing, building,
running, and analysis only. Do not try to work around this or ask the user to
disable it unless they bring it up; help them within the read tools available.

---

## What xmcp can do

xmcp gives you direct control over the Xojo IDE via 26 tools:

- **Navigate**: `list_project_items`, `get_current_location`, `select_project_item`
- **Read/write code**: `get_code`, `set_code`, `edit_code`, `get_selected_text`, `set_selected_text`
- **Build and run**: `build_project`, `run_project`, `stop_project`
- **Analyze**: `analyze_project` — compile-check the whole project or just the selected item without building
- **Create items**: `create_project_item`
- **Inspect and modify**: `get_item_description`, `constant_value`, `get_project_info`, `revert_project`, `save_project`
- **Debug session control**: `debug_control` — step over/into/out, resume, pause during a `run_project` session
- **IDE scripting**: `run_ide_script` (escape hatch for anything not covered)
- **Documentation**: `search_docs`, `lookup_class`, `list_doc_topics` *(require local docs — see below)*
- **Debugging**: `get_debug_log`, `get_system_log`
- **Cost estimation**: `estimate_request_cost` — call this proactively before broad or documentation-heavy tasks to check whether the approach is likely to be expensive, and to get suggestions for cheaper alternatives

---

## Documentation tools — setup required

The documentation tools (`search_docs`, `list_doc_topics`, `lookup_class`) require a local copy of the Xojo documentation. If they return "Xojo documentation path not configured", the user needs to run:

```sh
scripts/update-xojo-docs.sh
```

This downloads `llms.txt` and `llms-full.txt` from `docs.xojo.com` into the auto-detected path. Once downloaded, restart xmcp and the tools will work.

- `search_docs` — keyword search across the full documentation (`llms-full.txt`)
- `list_doc_topics` — browse/filter the topic index (`llms.txt`)
- `lookup_class` — look up a specific class by name from individual `_sources/*.rst.txt` files (fast, targeted reads)

---

## Starting work on a new project — recommended first steps

When you connect to a new Xojo project via xmcp:

1. Call `get_project_info` to confirm the IDE is connected and get the project directory path
2. Check whether `App` already has an `UnhandledException` handler (see below)
3. **If not, proactively offer to add it** — this is essential for diagnosing crashes in built apps

---

## Crash reporting — add UnhandledException to App

In built apps, runtime exceptions are silent unless you add an `UnhandledException` handler. Without it, crashes produce no output visible to xmcp.

Add this to `App.xojo_code` (before the `#tag ViewBehavior` section):

```xojo
#tag Event
    Sub UnhandledException(error As RuntimeException)
      Var msg As String = "Error: " + error.Message + EndOfLine
      msg = msg + "Error Number: " + Str(error.ErrorNumber) + EndOfLine
      If error.Stack <> Nil Then
        msg = msg + "Stack:" + EndOfLine
        For Each frame As String In error.Stack
          msg = msg + "  " + frame + EndOfLine
        Next
      End If

      Var f As New FolderItem("/tmp/xmcp_debug.log")
      Var stream As TextOutputStream = TextOutputStream.Open(f)
      stream.Write(msg)
      stream.Close
    End Sub
#tag EndEvent
```

After adding, ask the user for permission to call `revert_project` to reload the project.

Once in place, use `get_debug_log` after a crash in a built app to retrieve the full exception message and stack trace.

**Note:** `UnhandledException` does NOT fire in debug mode — the Xojo debugger intercepts exceptions first and shows them in the IDE.

---

## How to edit code — choose the right path first

Pick your approach based on what you're editing. Going down the wrong path always causes a break.

| What you're editing | How to do it |
| --- | --- |
| A small change to existing class / module / app-level code | `edit_code` with a dot-separated `location` (preferred) |
| Replacing a whole method / item body, or writing new code from scratch | `set_code` with dot-separated path |
| Window event handlers (`Opening`, `Close`, `Resized`, etc.) | Edit `.xojo_window` file directly on disk |
| Window layout, controls, or properties | Edit `.xojo_window` file directly on disk |

**Prefer `edit_code` for edits to existing code.** It replaces an exact
substring (`old_string` → `new_string`) in one call, so you don't have to
`get_code` the whole item, reconstruct it, and `set_code` it back. `old_string`
must match the current code exactly (whitespace and indentation included) and,
unless you pass `replace_all: true`, must occur exactly once — add surrounding
context to make it unique. Reach for `set_code` only when you're replacing an
entire body or writing something new.

**Window files cannot be edited through the IDE tools at all.** The IDE
scripting API only exposes the active code editor's text, which reaches
class/module/app-level code. It has no handle on window event handlers,
controls, or layout — so `get_code`, `set_code`, `edit_code`, and
`select_project_item` do not work on anything inside a `.xojo_window`. For those,
go straight to direct file editing (below); do not waste calls trying the IDE
tools first.

---

## Direct file editing — how to do it

1. **Find the project directory**
   Call `get_project_info` — it returns a `Project Directory:` line with the full path.

2. **Find the right file**
   - Classes, modules, app-level code → `<ClassName>.xojo_code`
   - Window UI, controls, and event handlers → `<WindowName>.xojo_window`
   - Project manifest → `<ProjectName>.xojo_project` (XML — edit sparingly)

3. **Edit the file — use your own editor, with exact-string edits**
   Use your native file-editing tool (the same one you'd use to edit any source
   file) to make a **targeted, exact-string replacement** in the file. Do **not**
   shell out to `sed`, `awk`, Python, or other text-munging scripts to rewrite
   the file — that is fragile and the most common way these edits go wrong. Read
   the file, match the exact block you're changing, and replace just that block.

   `.xojo_code` and `.xojo_window` are plain text with `#tag` markers. Follow the
   existing structure exactly and change as little as possible:

   - Keep every `#tag`/`#tag End…` block balanced and in its original order.
   - **Never invent or alter the hex IDs** in a `.xojo_window` — they must stay
     internally consistent, and fabricated IDs cause the IDE to crash. Only edit
     the code inside existing blocks; do not hand-author new control definitions.
   - Preserve indentation and the existing line structure verbatim outside your
     change.

   Window event handlers go in `#tag WindowCode`:

   ```xojo
   #tag WindowCode
       #tag Event
           Sub Opening()
             ' your code here
           End Sub
       #tag EndEvent
   #tag EndWindowCode
   ```

4. **Reload in the IDE**
   Ask the user for permission, then call `revert_project`. The user may see a confirmation prompt in the IDE — they need to accept it.

### Why this matters — two silent failure modes

The IDE's in-memory copy is authoritative while the project is open. This creates two traps with no error message:

- **Edit without revert → stale code runs.** If you modify a `.xojo_window` file but skip `revert_project`, the IDE keeps using its in-memory version. `run_project` and `build_project` silently use the old code — your edits have no effect and nothing reports a problem.
- **Save overwrites disk edits.** `save_project` (and Cmd+S in the IDE) writes the in-memory copy back to disk, clobbering any direct file edits. When a change doesn't seem to take effect, the instinct to "just save it" destroys the edit. Always `revert_project` (disk → IDE), never save, after editing files directly.

---

## IDE tool limitations to be aware of

### `select_project_item` cannot navigate to methods or events

The IDE scripting API can navigate to top-level items, classes, modules, and windows — but not to individual methods, properties, or event implementations.

For class-, module-, and app-level members, use `get_code` / `set_code` / `edit_code` with a full dot-separated path instead — these navigate automatically:

```text
get_code(location: "App.MyMethod")                ✓
edit_code(location: "Module1.Helper", ...)        ✓
select_project_item(item_path: "App.MyMethod")    ✗  (cannot target a method)
```

**Anything inside a window is the exception** — `Window1.Button1.Pressed`, other
control code, and layout are not reachable by the IDE tools through *any* path.
Edit the `.xojo_window` file directly (see "How to edit code" above).

`list_project_items` also does not list events — only methods, properties, and constants appear as children.

### Parallel tool calls are not supported

The Xojo IDE accepts only one IPC connection at a time. Always use sequential tool calls.

### IPC socket timing after navigation

After certain navigation operations, the IDE briefly closes its IPC socket (~2–3 seconds). xmcp retries automatically. If a tool times out immediately after navigation, retry once.

---

## Running and building — rules and workflow

### Never act without explicit user request

- **Never call `build_project` unless the user explicitly asks you to build**
- **Never call `run_project` unless the user explicitly asks you to run**
- **Never call `revert_project` without asking the user first** — it discards all unsaved changes in the IDE

Always wait for the user's answer before proceeding. Asking a question and then acting anyway defeats the purpose.

### Recommended workflow when the user asks to build

1. **Offer to run first**: Before building, offer to call `run_project` to catch syntax and runtime errors. Build does not catch all errors that run will catch.
2. **Run and ask for feedback**: After `run_project` returns, always ask the user if they see any errors or exceptions in the IDE — xmcp cannot see runtime behaviour in debug mode.
3. **Only build if run succeeds** — or if the user explicitly wants to build anyway.

### What run_project and build_project can and cannot see

| | `run_project` | `build_project` |
| --- | --- | --- |
| Syntax errors | ✓ Returns error | ✓ Returns error |
| Runtime exceptions (debug mode) | ✗ Invisible — IDE debugger catches them | — |
| Runtime exceptions (built app) | — | ✗ Invisible without `UnhandledException` |
| Build output on disk | — | ✓ Verify `.app` exists after build |

**After `run_project` returns "Project launched in debug mode"**: always ask the user if the app is behaving correctly and if they see any exceptions in the IDE debugger.

### build_project reliability

`build_project` may report "Build succeeded" without actually producing a build output. After a reported success, verify the `.app` exists on disk.

If no build output is found, use this reliable fallback:

1. Call `revert_project` (with user permission) to ensure the IDE has the latest files
2. Call `run_ide_script` with `DoCommand "BuildApp"`
3. Verify the `.app` exists on disk afterward

### Debug mode vs. built app — exception visibility

| Scenario | Exceptions visible to xmcp? | Where to look |
| --- | --- | --- |
| `run_project` (debug mode) | No | User sees them in Xojo IDE debugger |
| Built app with `UnhandledException` | Yes — via `get_debug_log` | `/tmp/xmcp_debug.log` |
| Built app without `UnhandledException` | No | Nowhere — add the handler |

### Tracing a running debug app — use System.DebugLog, not MessageBox

To trace logic in a running app (values, recursion, which branch fired), the
non-blocking path is:

1. Instrument the code with `System.DebugLog("…")` calls at the points you want
   to observe (via `edit_code` / `set_code`).
2. `run_project`, then exercise the behaviour.
3. Call `get_system_log` with `process_name` = `<AppName>.debug` to read the
   output.

**Do not reach for MessageBox** to diagnose. It blocks the UI thread, fires on
every iteration of a loop or recursive call, and requires a human to dismiss
each one — it derails exactly the kind of run you are trying to observe.
`System.DebugLog` has none of those problems.

`get_system_log` surfaces only `System.DebugLog` output, so **an empty result
almost always means no `System.DebugLog` calls ran — not that logging is
broken.** Instrument first, then query. If the app is clearly logging by some
other means, `get_system_log` will now also surface the raw `log show` entries
it found for the process (flagged as non-`System.DebugLog`), so you can see
*something* rather than a bare "nothing found."

---

## Tips for working effectively with xmcp

- Call `get_project_info` early to understand the project structure and get the directory path
- Use `list_project_items` to explore the project tree before navigating
- Use `run_ide_script` to run arbitrary IDE scripting commands when no dedicated tool exists. Treat it as a power tool: an IDE script can quit Xojo, close or modify the project, delete items, and invoke shell commands via `DoShellCommand`. Prefer a dedicated tool when one fits, and pause before running anything destructive.
- Use `get_system_log` to retrieve `System.DebugLog` output — works for both debug builds (`AppName.debug`) and built apps (`AppName`)

---

*This file can be edited to add project-specific notes, custom conventions, or additional guidance for your AI assistant.*
