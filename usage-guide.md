# xmcp Usage Guide for AI Assistants

This file is automatically loaded as an MCP resource when you connect to xmcp. It describes xmcp's capabilities, known limitations, and how to choose the right approach for each task. You can edit this file to add project-specific notes or customise the guidance.

---

## Prerequisites — before using any xmcp tools

**xmcp cannot start Xojo IDE.** All tools communicate via a macOS domain socket (`/tmp/XojoIDE`) that Xojo IDE creates when it launches. If the IDE is not running, every tool call will fail with "IPC socket not found".

If several Xojo IDEs are running side by side, each was started with its own `XOJO_IPCPATH` name and xmcp connects only to the instance whose name it was given in its own environment — so "IPC socket not found" from a session with multiple IDEs open usually means xmcp and the intended IDE disagree on that value, not that the IDE is down.

**The user must:**

1. Start Xojo IDE manually
2. Open the project they want to work with (File > Open) — xmcp cannot open projects
3. Wait a few seconds after launch before the IPC socket is ready — if tools fail immediately after IDE start, ask the user to wait and retry

**Do not attempt any xmcp tool calls until the user confirms that Xojo IDE is open and the project is loaded.**

---

## Read-only mode

xmcp may be running in **read-only mode** (started with `--read-only` or
`XMCP_READ_ONLY=1`). When it is, the tools that modify the project —
`set_code`, `edit_code`, `set_selected_text`, `set_declaration`,
`create_project_item`, `property_value`, `constant_value`,
`get_item_description`, `revert_project`, and `save_project` — are not listed
and cannot be called. If you don't see those
tools, the user has intentionally opened the project for browsing, building,
running, and analysis only. Do not try to work around this or ask the user to
disable it unless they bring it up; help them within the read tools available.

---

## What xmcp can do

xmcp gives you direct control over the Xojo IDE via 31 tools:

- **Navigate**: `list_project_items`, `get_current_location`, `select_project_item`
- **Read/write code**: `get_code`, `set_code`, `edit_code`, `get_selected_text`, `set_selected_text`
- **Build and run**: `build_project`, `run_project`, `stop_project`, and `agentic_run` — run an Agentic Mode project and get its real compile errors back (Xojo 2026r3+, see "Agentic Mode projects")
- **Analyze**: `analyze_project` — compile-check the whole project or just the selected item without building
- **Create items**: `create_project_item`, then `set_declaration` to name and type it — `NewProperty` and `NewMethod` create a *blank, untyped* member, and `set_declaration` is the half that gives it a name, type, scope and parameters. The two work as consecutive tool calls: the newly created item stays current between them, so `set_declaration` needs no `location`. Verified on Xojo 2026r2.1 — `create_project_item` reported `Window1.Untitled`, and a separate `set_declaration` call turned it into `Window1.UserName`. Pass `location` only when editing the declaration of something that already exists
- **Inspect and modify**: `get_item_description`, `constant_value`, `get_project_info`, `revert_project`, `save_project`
- **App and build settings**: `property_value` — reads and writes *framework* properties such as `App.OptimizationLevel`, `App.SupportsDarkMode` or `Window1.Title`. This is the supported way to change settings that live in the `.xojo_project` file, which must never be hand-edited. It cannot reach properties you added yourself, and it cannot address controls on a window (there is no `Window1.PushButton1.Caption`) — for control properties, edit the `.xojo_window` file directly and `revert_project`
- **Debug session control**: `debug_control` — step over/into/out, resume, pause during a `run_project` session
- **Project files on disk**: `scaffold_code_block` writes a correctly formatted `#tag` block to insert into a `.xojo_code` or `.xojo_window` file, and `lint_project_file` checks such a file after you edit it. Neither needs the IDE — see "Xojo file structure rules"
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
- `lookup_class` — look up a specific class by name from individual `_sources/*.rst.txt` files (fast, targeted reads). By default it returns the page's description plus its member tables, with Sphinx cross-reference markup resolved to plain text — a few thousand characters rather than the tens of thousands the raw page holds. Pass `member` for a single member's entry (`lookup_class(class_name: "String", member: "Middle")`), which is what to reach for once you know the name you want. Pass `full: true` only if the shaped output seems to have dropped something: the raw page for `String` is over 70,000 characters of reStructuredText and will consume the context it lands in.

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
| Window event handlers (`Opening`, `Close`, `Resized`, etc.) on Xojo 2026r3 or later | `edit_code` / `set_code` with `location` like `Window1.Opening` |
| Control event handlers on Xojo 2026r3 or later | `edit_code` / `set_code` with `location` like `Window1.Controls.Button1.Pressed` |
| Window and control event handlers on earlier releases | Edit `.xojo_window` file directly on disk |
| Window layout, controls, or properties | Edit `.xojo_window` file directly on disk |

**Prefer `edit_code` for edits to existing code.** It replaces an exact
substring (`old_string` → `new_string`) in one call, so you don't have to
`get_code` the whole item, reconstruct it, and `set_code` it back. `old_string`
must match the current code exactly (whitespace and indentation included) and,
unless you pass `replace_all: true`, must occur exactly once — add surrounding
context to make it unique. Reach for `set_code` only when you're replacing an
entire body or writing something new.

Build `old_string` by **copying from `get_code` output verbatim** — do not
retype it from memory. Xojo indents method bodies with tabs, so a retyped
guess at the indentation will not match. If an edit fails, `edit_code` detects a
whitespace-only-differing match and hands you back the real text to retry with,
so copy that exactly rather than guessing again.

**Window files are only partly reachable through the IDE tools.** The IDE
scripting API only exposes the active code editor's text. On Xojo 2026r3 that
includes event handlers — `get_code`, `set_code` and `edit_code` read and write
`Window1.Opening` and `Window1.Controls.Button1.Pressed` (verified on 2026r3;
note the `Controls` segment). It has no handle on controls themselves or on
layout, so for those — and for event handlers on earlier releases — go straight
to direct file editing (below) rather than spending calls on the IDE tools
first.

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
   - To add a method, property, constant, event definition or event handler,
     get its text from `scaffold_code_block` instead of writing `#tag` syntax
     from memory — see "Xojo file structure rules" below.

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

4. **Check the file**
   Call `lint_project_file` with the file's absolute path. Fix every error it
   reports before going on — an unbalanced `#tag` pair breaks the file. Fix the
   warnings too unless you have a reason not to: Xojo loads a misordered block or a
   Flags/keyword mismatch, but resolves the mismatch on its own terms, and an
   unescaped constant value can be silently truncated.

5. **Reload in the IDE**
   Ask the user for permission, then call `revert_project`. The user may see a confirmation prompt in the IDE — they need to accept it.
   **Skip this step in an Agentic Mode project** — Xojo reloads the file by itself (see "Agentic Mode projects" below).

### Why this matters — two silent failure modes

Outside Agentic Mode, the IDE's in-memory copy is authoritative while the project is open. This creates two traps with no error message:

- **Edit without revert → stale code runs.** If you modify a `.xojo_window` file but skip `revert_project`, the IDE keeps using its in-memory version. `run_project` and `build_project` silently use the old code — your edits have no effect and nothing reports a problem.
- **Save overwrites disk edits.** `save_project` (and Cmd+S in the IDE) writes the in-memory copy back to disk, clobbering any direct file edits. When a change doesn't seem to take effect, the instinct to "just save it" destroys the edit. Always `revert_project` (disk → IDE), never save, after editing files directly.

---

## Xojo file structure rules

`scaffold_code_block` writes `#tag` blocks by these rules and `lint_project_file`
checks a file against them. Both read the rules from the JSON block at the end of
this section, so correcting a rule there takes effect on the next call, with no
rebuild. Where a rule below is marked unconfirmed, the tools deliberately do not
enforce it.

### Block order

Xojo writes blocks in a fixed order, but it is forgiving when reading them: tested
on Xojo 2026r3, a module with its members out of order — even one placed after
`#tag ViewBehavior` — loads with nothing lost and is re-sorted on the next save.
So `lint_project_file` reports order as a warning, not an error. Keep to the order
anyway; it keeps diffs small.

Member blocks in a **class** file come in this order, and `#tag ViewBehavior` is
always last:

1. `#tag Event` — handlers for events the class inherits
2. `#tag Method`
3. `#tag Property` and `#tag ComputedProperty`, interleaved
4. `#tag Constant`
5. `#tag ViewBehavior`

`#tag Note`, `#tag Hook`, `#tag Enum` and `#tag Structure` are not checked:
notes appear in several places in IDE-written files, and where the IDE puts the
others has not been confirmed.

A **module** file has the same order without the event handlers: `#tag Method`,
then `#tag Property`, then `#tag Constant`, then `#tag ViewBehavior`. Upstream
XMCP documents constants before properties; that is not what Xojo writes — a
2026r3 module whose constant was created first, then a property, then a method
was saved as method, property, constant.

A **window** file has `#tag DesktopWindow` (the layout), then `#tag WindowCode`
(window events, methods, properties), then one `#tag Events ControlName` block per
control that handles events, then `#tag ViewBehavior`.

### Flags and visibility keywords

Every `#tag Method` and `#tag Property` line carries `Flags = &hN`, and the
declaration on the next line must start with the matching keyword:

| Flag | Visibility | Declaration starts with |
| --- | --- | --- |
| `&h0` | public (global, in a module) | no keyword |
| `&h1` | protected | `Protected` |
| `&h21` | private | `Private` |

A shared method keeps the same flag and adds `Shared` after the keyword:
`Private Shared Function Create() As MyClass`.

When the two disagree, Xojo still loads the file, and which one it believes
depends on whether the keyword is there (tested on 2026r3): a declaration **with**
a keyword wins, and the Flags are rewritten to match on the next save; a
declaration **without** one takes the Flags' visibility, and the keyword is added
on save. So a missing `Private` keeps a member private, but a stray `Private` on a
member flagged `&h0` makes it private. `lint_project_file` warns about either case
and says which visibility Xojo will use.

### Event definitions and event handlers

`#tag Hook` *defines* a new event that a class raises with `RaiseEvent`; it holds
only the signature. `#tag Event` *handles* an event — one the class inherits, a
window event inside `#tag WindowCode`, or a control's event inside
`#tag Events ControlName`. Writing `#tag Event` to define a new event is wrong.

### Constant Default values

All of a constant lives on its `#tag Constant` line. The value sits between an
escaped opening quote and a raw closing quote — `Default = \"100", Scope = Public` —
and inside it:

| Character | Written as |
| --- | --- |
| `"` | `\"` |
| `'` | `\'` |
| `=` | `\x3D` |
| `,` | `\x2C` |
| newline | `\n` |
| non-ASCII, e.g. `°` | its UTF-8 bytes, `\xC2\xB0` — never `\uXXXX` |

An unescaped character silently truncates the value; Xojo reports no error. A
value that opens with a raw `"` instead of `\"` also compiles, but Xojo reads it
back without its first character. How a backslash, a tab or another control
character is escaped has not been confirmed, so `scaffold_code_block` refuses
them — set such a value with `constant_value`, which lets Xojo do the escaping.

### Format Rules (machine-readable — keep this block valid JSON)

In `block_order`, a step written as a list holds kinds that may be mixed in any
order. Kinds not listed are not checked.

```json
{
  "flags": {
    "&h0": {"keyword": "", "visibility": "public"},
    "&h1": {"keyword": "Protected", "visibility": "protected"},
    "&h21": {"keyword": "Private", "visibility": "private"}
  },
  "block_order": {
    "xojo_code_class": ["Event", "Method", ["Property", "ComputedProperty"], "Constant", "ViewBehavior"],
    "xojo_code_module": ["Method", ["Property", "ComputedProperty"], "Constant", "ViewBehavior"],
    "xojo_window": ["DesktopWindow", "WindowCode", "Events", "ViewBehavior"]
  },
  "constant_escape": {
    "'": "\\'",
    "=": "\\x3D",
    ",": "\\x2C",
    "\n": "\\n"
  },
  "constant_types": ["String", "Integer", "Double", "Boolean", "Color"]
}
```

---

## Agentic Mode projects (Xojo 2026r3+)

In Agentic Mode (File > Agentic Mode in Xojo 2026r3 and later) the IDE watches
the project folder and reloads any file that changes on disk. `get_project_info`
reports `Agentic Mode: on` when the saved project has it enabled. The rules
change in four ways:

- **No `revert_project` after a file edit.** The reload happens by itself,
  within a few seconds. Do not call `revert_project` for it.
- **A disk edit wins, silently, for the whole file.** The reload replaces
  everything the IDE holds for that file and discards any unsaved IDE edit in it
  — in *any* member, not just the one you changed, and with no prompt. Verified
  on 2026r3: a disk edit to a constant in `App.xojo_code` wiped an unsaved edit
  to a method in the same file. Two consequences:
  - Use one channel per file. If you changed items with `set_code` /
    `edit_code` and now need to edit the same file on disk, call `save_project`
    first, or your IDE-side changes are lost.
  - The user's unsaved typing is just as exposed. Before editing a file on disk
    that the user may have open, ask them to save.
- **Compile with `agentic_run`.** It asks Xojo to run the project and returns
  the real compiler errors, with locations; `run_project` cannot see those.
  Agentic Mode is the user's opt-in to having the agent run the project to check
  its work, so `agentic_run` may be used to verify your edits without asking
  each time. A clean run launches the app in the debugger: stop it with
  `stop_project` unless the user wants it running. Error lines read
  `<location>, line <n>: <message>`; line numbers count from the first line of
  the method body, and control events are named without the `Controls` segment
  (`Window1.Button1.Pressed`), which the IDE tools need
  (`Window1.Controls.Button1.Pressed`).
- **New project items still go through the IDE.** Xojo's own `XOJO-CODING.md`
  tells agents to add new items to the `.xojo_project` manifest by hand. Don't:
  use `create_project_item`, which lets the IDE assign the item's ID. The rule
  that the manifest is never hand-edited stands in Agentic Mode.

---

## IDE tool limitations to be aware of

### Which paths the IDE tools can reach

Every tool that takes a location navigates there first, and refuses to run if
the IDE did not land exactly on the path you gave — the error names where it did
land, so a near miss tells you the right spelling. Use full dot-separated paths:

```text
get_code(location: "App.MyMethod")                       ✓
edit_code(location: "Module1.Helper", ...)               ✓
get_code(location: "Window1.Opening")                    ✓  2026r3+
get_code(location: "Window1.Controls.Button1.Pressed")   ✓  2026r3+
get_code(location: "Window1.Button1.Pressed")            ✗  needs the Controls segment
```

On releases before 2026r3, window and control event handlers and layout are not
reachable through any path — edit the `.xojo_window` file directly (see "How to
edit code" above). Layout and control properties are never reachable.

`list_project_items` does not list events — only methods, properties, and constants appear as children.

### IDE scripts: how `run_ide_script` answers

The IDE answers once per `Print`, and not at all for a script without one. xmcp
appends a final `Print` of its own to every script, so a script that prints
nothing still answers ("The script ran but produced no value"), and that last line
is how xmcp knows the whole answer has arrived. Two consequences:

- **Print once.** With several `Print`s you get the first one that says
  something; a compile or runtime error outranks printed output.
- **Do not leave a script early with `Return`.** It never reaches xmcp's final
  line, so xmcp waits for the full timeout before giving up on it.

A runtime error comes back with no message and no line number — the IDE gives
none. To find it, split the script or `Print` values before the line you suspect.

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

In an Agentic Mode project, `agentic_run` sees what `run_project` cannot: it
returns the compiler's error list, with locations, instead of a bare launch
message. Runtime exceptions stay invisible to both.

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
