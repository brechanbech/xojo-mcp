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

## What xmcp can do

xmcp gives you direct control over the Xojo IDE via 25 tools:

- **Navigate**: `list_project_items`, `get_current_location`, `select_project_item`
- **Read/write code**: `get_code`, `set_code`, `get_selected_text`, `set_selected_text`
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
| Class / module / app-level code (`.xojo_code`) | `get_code` / `set_code` with dot-separated path |
| Window event handlers (`Opening`, `Close`, `Resized`, etc.) | Edit `.xojo_window` file directly on disk |
| Window layout, controls, or properties | Edit `.xojo_window` file directly on disk |

**For window files: go straight to direct file editing — do not try IDE tools first.**

---

## Direct file editing — how to do it

1. **Find the project directory**
   Call `get_project_info` — it returns a `Project Directory:` line with the full path.

2. **Find the right file**
   - Classes, modules, app-level code → `<ClassName>.xojo_code`
   - Window UI, controls, and event handlers → `<WindowName>.xojo_window`
   - Project manifest → `<ProjectName>.xojo_project` (XML — edit sparingly)

3. **Edit the file**
   `.xojo_code` and `.xojo_window` are plain text with `#tag` markers. Follow the existing structure exactly.

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

Use `get_code` / `set_code` with a full dot-separated path instead — these navigate automatically:

```text
get_code(location: "Window1.Button1.Pressed")   ✓
set_code(code: "...", location: "App.MyMethod") ✓
select_project_item(item_path: "App.MyMethod")  ✗
```

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

---

## Tips for working effectively with xmcp

- Call `get_project_info` early to understand the project structure and get the directory path
- Use `list_project_items` to explore the project tree before navigating
- Use `run_ide_script` to run arbitrary IDE scripting commands when no dedicated tool exists. Treat it as a power tool: an IDE script can quit Xojo, close or modify the project, delete items, and invoke shell commands via `DoShellCommand`. Prefer a dedicated tool when one fits, and pause before running anything destructive.
- Use `get_system_log` to retrieve `System.DebugLog` output — works for both debug builds (`AppName.debug`) and built apps (`AppName`)

---

*This file can be edited to add project-specific notes, custom conventions, or additional guidance for your AI assistant.*
