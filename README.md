# xojo-mcp

An MCP (Model Context Protocol) server that gives AI assistants direct control
over the Xojo IDE. Communicates via stdin/stdout JSON-RPC and forwards IDE
commands via a Unix domain socket to the running Xojo IDE process.

> **Note on the name.** This crate was previously published as `xmcp`. It was
> renamed to `xojo-mcp` to avoid confusion with X's (formerly Twitter)
> unrelated `xmcp` framework. The installed **binary is still `xmcp`**, so
> existing configs keep working.

> **macOS only.** xojo-mcp talks to the Xojo IDE over a Unix domain socket
> (`/tmp/XojoIDE`, or `/tmp/$XOJO_IPCPATH` — see
> [Choosing which IDE to talk to](#choosing-which-ide-to-talk-to)). Windows is
> not supported and there is no plan to add it. The reason is not that the IDE
> lacks an IPC mechanism there: Xojo's `IPCSocket` is a framework abstraction,
> and on Windows it is a localhost TCP socket whose port is derived from the path
> string rather than a filesystem entry. The original
> [XMCP](https://github.com/o3jvind/XMCP), being itself a Xojo application, gets
> that transport for free from the framework; this port speaks the wire protocol
> directly, so Windows would mean reverse-engineering an undocumented
> path-to-port derivation. Linux is untested rather than ruled out — it uses a
> Unix socket as macOS does, but the debug- and system-log tools read macOS-only
> sources.

## Attribution

This is a Rust port of [XMCP](https://github.com/o3jvind/XMCP) by
Øjvind Søgaard Andersen, originally written in Xojo. The original project
is licensed under the MIT License.

## Quick start

### 1. Install

#### Option A: prebuilt binary (Apple Silicon)

Each release carries an `aarch64-apple-darwin` tarball that is signed with a
Developer ID certificate, notarized by Apple, and accompanied by a detached
[minisign](https://jedisct1.github.io/minisign/) signature. Grab the latest from
[Releases](https://codeberg.org/brechanbech/xojo-mcp/releases), check it, and put
the binary on your `PATH`:

```sh
minisign -Vm xmcp-*-aarch64-apple-darwin.tar.gz \
    -P RWR7Jy43Mphvu+jrP2FynfYpR7WdP0PvQaSFKfXub7q9Sh7fjtyAX9GU
tar xzf xmcp-*-aarch64-apple-darwin.tar.gz
install -m 755 xmcp-*/xmcp ~/.cargo/bin/xmcp
```

The `.minisig` is the check worth doing, and the one the `.sha256` beside it
cannot do: a checksum file hosted next to the file it vouches for proves only
that the download wasn't corrupted, since anyone able to replace one asset can
replace both. The signature is made by a key that never touches the release
server — the one in the command above, kept in this repository as
`minisign.pub`.

`minisign` is in MacPorts (`port install minisign`). A successful verify prints
the trusted comment, which names the version and the date it was signed. The
`.sha256` is still published for anyone who wants it:

```sh
shasum -a 256 -c xmcp-*-aarch64-apple-darwin.tar.gz.sha256
```

The binary is signed with a Developer ID certificate and notarized by Apple, so
Gatekeeper lets it run as-is. Check that for yourself if you like:

```sh
codesign -dvv ~/.cargo/bin/xmcp   # Authority: Developer ID Application …
codesign -vvv -R="notarized" --check-notarization ~/.cargo/bin/xmcp
```

(`spctl` is the obvious thing to reach for and the wrong tool here: it answers
`does not seem to be an app` for any plain command-line binary, notarized or
not.)

A notarization ticket cannot be stapled to a bare executable — `stapler` only
handles `.app`, `.dmg` and `.pkg` — so the *first* run needs to reach Apple to
resolve the ticket. If that first run has to happen offline, clear the
quarantine flag by hand instead:

```sh
xattr -d com.apple.quarantine ~/.cargo/bin/xmcp
```

Intel Macs are not covered — build from source instead.

#### Option B: from crates.io

```sh
cargo install xojo-mcp
```

The crate is `xojo-mcp`; the binary it installs is `xmcp`.

#### Option C: from source

```sh
git clone https://codeberg.org/brechanbech/xojo-mcp.git
cd xojo-mcp
cargo install --path .
```

This installs the `xmcp` binary to `~/.cargo/bin/xmcp`. If `~/.cargo/bin`
is already on your `PATH` (the Rust installer adds it by default), you're
done. Verify with:

```sh
xmcp --help
```

**Optional:** `usage-guide.md` is embedded into the binary at compile
time, so the MCP resource is always available out of the box. If you
want to tweak the guide without rebuilding, drop a copy next to the
binary — xmcp will prefer the file on disk over the embedded fallback:

```sh
cp usage-guide.md ~/.cargo/bin/
```

### 2. Add to Claude Code

Run this from any terminal:

```sh
claude mcp add xmcp -- xmcp
```

Or add it manually to your Claude Code settings. Open the MCP config file
(on macOS: `~/.claude/settings.json` or the project-level
`.claude/settings.json`) and add:

```json
{
  "mcpServers": {
    "xmcp": {
      "command": "xmcp",
      "args": []
    }
  }
}
```

To enable verbose logging (written to stderr, visible in the Claude Code
MCP log):

```json
{
  "mcpServers": {
    "xmcp": {
      "command": "xmcp",
      "args": ["-v"]
    }
  }
}
```

### 3. Use it

1. Start the Xojo IDE and open your project
2. Start a Claude Code session in the project directory
3. Claude will automatically discover the 31 xmcp tools and the usage guide

### 4. Download Xojo documentation (recommended)

The documentation tools (`search_docs`, `lookup_class`, `list_doc_topics`) need
a local copy of the Xojo docs. A script is included to download them from
`docs.xojo.com`:

```sh
scripts/update-xojo-docs.sh
```

This downloads `llms.txt` and `llms-full.txt` from `docs.xojo.com` and splits
the full documentation into individual class files under `_sources/`. Everything
goes into `~/Library/Application Support/Xojo/Xojo/<version>/Documentation/`,
which xmcp auto-detects at startup. Re-run the script periodically to pick up
documentation updates — Xojo refreshes these files when new releases are
published.

`lookup_class` returns a shaped summary by default: the class description plus
its member tables, with Sphinx cross-reference markup resolved to plain text.
That is a few thousand characters where the raw page runs to tens of thousands
— `String` alone is over 70,000. Ask for one member with `member`, or the
unprocessed page with `full: true`:

```
lookup_class(class_name: "String")                     # description + member tables
lookup_class(class_name: "String", member: "Middle")   # that member's entry alone
lookup_class(class_name: "String", full: true)         # the raw .rst.txt page
```

To use a custom location instead:

```sh
scripts/update-xojo-docs.sh /path/to/docs
xmcp --docs-path /path/to/docs
```

## Read-only mode

By default, an assistant connected to xmcp can change your project: it can
rewrite code, create new items, save, and revert. That is the point of the tool
— but it is not always what you want. If you only want an assistant to *look
at* a project — read the code, build it, run it, analyse it, answer questions,
consult the documentation — without any possibility of it modifying or
overwriting your source, start the server in **read-only mode**.

Read-only mode is enforced by the server itself, not by asking the assistant to
behave. It is the single most important safety control in xmcp, so it is worth
understanding exactly how it works.

### Enabling it

The simplest way is the `--read-only` flag on the launch command. In Claude
Code:

```sh
claude mcp add xmcp -- xmcp --read-only
```

Or, in a Claude Code settings file (`~/.claude/settings.json`, or the
project-level `.claude/settings.json`):

```json
{
  "mcpServers": {
    "xmcp": {
      "command": "xmcp",
      "args": ["--read-only"]
    }
  }
}
```

If your MCP client prefers environment variables to command-line arguments, the
variable `XMCP_READ_ONLY=1` does exactly the same thing. Accepted truthy values
are `1`, `true`, `yes`, and `on` (case-insensitive):

```json
{
  "mcpServers": {
    "xmcp": {
      "command": "xmcp",
      "args": [],
      "env": { "XMCP_READ_ONLY": "1" }
    }
  }
}
```

If both the flag and the environment variable are present, either one enabling
read-only mode is enough — there is no way to *disable* it from the other.

### It is set at launch, not in the conversation

This is the part newcomers most often get wrong. **You do not put the assistant
into read-only mode by telling it to "stick to reading" in the chat.** A prompt
is a request the model can forget, misinterpret, or be argued out of — and it
does nothing at all if the model simply calls a write tool anyway. That is the
weakness read-only mode exists to remove.

Read-only mode is a property of *how the server was started*. You set it once,
in your MCP client's configuration, before the session begins. From that point
on, for the entire lifetime of that server process, the restriction holds
regardless of anything typed into the conversation. Neither you nor the
assistant can toggle it mid-session; to change modes you change the
configuration and reconnect the server. (Restarting the server is required for a
change to take effect — a server that is already running will not pick up a new
flag or environment variable.)

### What it actually blocks

Read-only mode disables the ten tools that modify the project:

| Tool | What it would otherwise do |
| --- | --- |
| `set_code` | Overwrite the code of a method, property, or other item |
| `edit_code` | Replace an exact substring within an item's code |
| `set_selected_text` | Replace the current text selection in the code editor |
| `set_declaration` | Rename or retype a method or property |
| `create_project_item` | Add a new class, module, window, or other item |
| `property_value` | Change a framework property such as `App.OptimizationLevel` |
| `constant_value` | Change a project constant's value |
| `get_item_description` | Change an item's description |
| `revert_project` | Discard unsaved changes back to the last save |
| `save_project` | Write the project's current in-memory state to disk |

`property_value`, `constant_value` and `get_item_description` also *read*, but each
is one tool, so read-only mode blocks the read along with the write.

### Tool annotations

Every tool also carries MCP behaviour hints in `tools/list` — a `title` and
`readOnlyHint`, `destructiveHint`, `idempotentHint` and `openWorldHint` — so a
client can decide what to confirm before calling it. These are hints, not
enforcement, and they describe a tool's effect on the whole machine rather than
on the project, so they are stricter than read-only mode: `build_project`,
`run_project` and `agentic_run` leave the source alone and stay available in
read-only mode, but they write build output or launch an app, so they are not
hinted read-only. `openWorldHint` is false everywhere except `run_ide_script`,
whose `DoShellCommand` can reach anything.

Everything else remains fully available — navigating and listing items, reading
code (`get_code`), building (`build_project`), running (`run_project`),
stopping, compile-checking (`analyze_project`), inspecting descriptions and
constants, the debug log tools, and all three documentation tools. In short:
browse, build, run, and analyse — just no writing.

Note that build and run are deliberately *not* blocked. They do not alter your
source; they exercise it. `build_project` writes a compiled app into the build
folder and `run_project` launches a debug session, but neither touches the
project itself, so both are considered read-only-safe.

### How the enforcement works

The restriction is applied in two independent layers, so it holds even if a
client or model misbehaves:

1. **The blocked tools are removed from the tool list.** When the assistant asks
   the server what tools exist (`tools/list`), the six mutating tools are
   filtered out. The model never sees them, so it cannot choose to call
   something it does not know exists. This is what makes the mode effective in
   practice rather than merely defensive.
2. **Any call to a blocked tool is rejected.** If a request to one of the six
   arrives anyway — a stale tool list, a hand-crafted call, a buggy client — the
   server refuses it before the request ever reaches the IDE, returning a clear
   error explaining that the tool is disabled in read-only mode.

The assistant is also told, via the embedded usage guide, that when it sees a
reduced tool set the project is intentionally read-only and it should work
within the available tools rather than trying to route around the restriction.

### Running both modes at once

Because the mode is fixed per server, you can register xmcp twice under
different names and choose per task which one to point the assistant at — for
example a normal `xmcp` for editing sessions and a separate `xmcp-ro` for
review-only sessions:

```sh
claude mcp add xmcp    -- xmcp
claude mcp add xmcp-ro -- xmcp --read-only
```

## Choosing which IDE to talk to

The IDE listens on a socket whose path is a temporary directory plus the name
`XojoIDE` — or the value of `XOJO_IPCPATH`, when that variable is set in the
IDE's environment. Setting it to a unique name per instance is Xojo's supported
way of running several IDEs (say, two releases side by side) and addressing each
one independently:

```sh
env XOJO_IPCPATH=Xojo2026r2_1 "/Applications/Xojo 2026 Release 2.1/Xojo.app/Contents/MacOS/Xojo" &
```

xmcp must be given the same value, otherwise it connects to whichever instance
owns the default `XojoIDE` socket:

```json
{
  "mcpServers": {
    "xmcp-2026r2-1": {
      "command": "xmcp",
      "args": [],
      "env": { "XOJO_IPCPATH": "Xojo2026r2_1" }
    }
  }
}
```

Only `a-z`, `A-Z`, `0-9` and `_` are valid in the name; xmcp ignores a value
containing anything else (with a warning on stderr) and falls back to `XojoIDE`.
Candidate directories are `/tmp` first — what the IDE itself prefers — then
`$TMPDIR`, which is where the IDE falls back when `/tmp` is not writable.

If you only ever run one IDE at a time, ignore all of this: the default works.

## Requirements

- macOS (the Xojo IDE IPC socket is macOS-specific)
- Rust toolchain (`rustup` — https://rustup.rs)
- Xojo IDE must be running with a project open before using any tools

## Options

```
xmcp [OPTIONS]
```

- `--read-only` — Read-only mode: hide and reject every tool that modifies the
  project. Can also be enabled with `XMCP_READ_ONLY=1`. See
  [Read-only mode](#read-only-mode) for the full description.
- `-v`, `--verbose` — Enable verbose logging to stderr
- `-d`, `--docs-path <PATH>` — Path to Xojo documentation directory (auto-detected if omitted)
- `-V`, `--version` — Print version
- `-h`, `--help` — Print help

Environment: `XMCP_READ_ONLY` (see above) and `XOJO_IPCPATH`, which selects the
IDE instance to connect to — see
[Choosing which IDE to talk to](#choosing-which-ide-to-talk-to).

## Differences from the original

This is a drop-in replacement — it exposes all 25 tools from the original with
identical names and parameters (`analyze_project` and `debug_control` were the
last two ported, bringing it to full parity), plus one new tool, `edit_code`,
for 26 in total. Same IDE Communicator Protocol v2 over the Unix domain socket,
updated to MCP protocol version `2025-11-25`.

Notable differences:

- **Binary name** is `xmcp`
- **`edit_code` — targeted str_replace-style editing** — replaces an exact
  substring within an item's code in one call (read → replace → write, all
  server-side), instead of resending the whole item via `set_code`. The
  original has no such tool, forcing whole-item rewrites or shell-based text
  munging for small edits.
- **Writes code directly — no shell/base64 marshalling** — `set_code` and
  `edit_code` send source straight through the IPC, with all IDE-script string
  escaping (quotes, newlines, special characters) handled server-side. There is
  no need to smuggle code across the bridge by hand — the base64
  encode-on-agent / decode-on-Mac / run-via-shell workflow the original forces
  for file edits simply does not exist here.
- **Enforced read-only mode** — `--read-only` / `XMCP_READ_ONLY` removes and
  rejects the mutating tools at the server. The original has no built-in
  enforcement; it can only be asked, via the prompt, not to write. See
  [Read-only mode](#read-only-mode).
- **`scaffold_code_block` and `lint_project_file` differ from the original's in a few
  places.** A guide on disk that predates their rules falls back to the compiled-in rules
  instead of failing. The scaffold takes a full signature, so a return type yields a
  `Function`, and it takes a body. It refuses a constant value whose escaping is
  unconfirmed (a backslash or control character) instead of guessing. The lint blames the
  block that lost its end tag instead of cascading. Its rules were re-checked against Xojo
  2026r3: a module goes method, property, constant (the original has constants before
  properties), and since Xojo loads misordered blocks and Flags/keyword mismatches and
  resolves them on save, the lint reports those as warnings that say what Xojo will do,
  not as errors.
- **No Xojo license required** — builds with the standard Rust toolchain
- **usage-guide.md has a compiled-in fallback** — the original fails silently
  if the file is missing next to the binary; the Rust version embeds a copy
  at compile time so the MCP resource is always available. A file on disk
  still takes priority, so you can edit it without rebuilding.
- **CLI parsing** uses [clap](https://crates.io/crates/clap) rather than the
  original's custom OptionParser. The flags are the same.

## Tools

xmcp exposes 31 tools across six categories:

**IDE tools (22):** list_project_items, get_current_location, select_project_item,
get_code, set_code, edit_code, get_selected_text, set_selected_text, build_project,
run_project, stop_project, create_project_item, run_ide_script, get_project_info,
revert_project, save_project, get_item_description, constant_value, property_value,
set_declaration, analyze_project, debug_control

**Documentation tools (3):** search_docs, lookup_class, list_doc_topics

**Debug tools (2):** get_debug_log, get_system_log

**Agentic Mode (1):** agentic_run — runs a project that has Xojo 2026r3's Agentic Mode on and
returns its compile errors with locations. It works through files in the project folder, not
the IDE socket, so it can be pointed at a project with `project_dir` even when the socket is
unavailable.

**Project-file tools (2):** scaffold_code_block, lint_project_file — write a correctly
formatted `#tag` block for a `.xojo_code` or `.xojo_window` file, and check such a file after
editing it on disk. They read the file only, need no IDE, and take their rules from the JSON
block in `usage-guide.md`, so a rule can be corrected without a rebuild.

**Cost awareness (1):** estimate_request_cost

## License

MIT — see [LICENSE.md](LICENSE.md) for details.

## MCP registry

Ownership-verification token for the [MCP registry](https://registry.modelcontextprotocol.io)
(read from this crate's rendered README on crates.io):

> Registry ownership token: `mcp-name: io.github.brechanbech/xojo-mcp`

### Releasing

A version is immutable once published — crates.io yanks, it does not delete, and a number is
never reused — so the order matters:

1. `cargo test` and `cargo clippy --all-targets` clean.
2. Bump `version` in **`Cargo.toml`**.
3. Bump `version` in **`server.json`** — *both* fields, the top-level one and the one under
   `packages[]`. Nothing enforces this: no script, no CI, no test reads the file. It sat at
   3.4.0 through the 3.4.1 and 3.5.0 releases because of it, leaving the registry advertising a
   version two behind what anyone could install.
4. Commit and push; let CI go green before publishing rather than after.
5. `cargo publish --dry-run`, which compiles *from the packaged copy* and so catches a crate
   depending on a file it does not ship. Then `cargo publish`.
6. Publish to the MCP registry, **after** crates.io and not before — the registry verifies the
   namespace by reading `mcp-name` from this crate's *rendered README on crates.io*, and
   `server.json` names a cargo version that must already exist there:

   ```sh
   mcp-publisher validate      # checks server.json against the live registry
   mcp-publisher login github  # interactive, for the io.github.brechanbech/* namespace
   mcp-publisher publish 
   ```

[![MCP Badge](https://lobehub.com/badge/mcp/brechanbech-xojo-mcp?style=plastic)](https://lobehub.com/mcp/brechanbech-xojo-mcp)
[![xojo-mcp MCP server – quality and maintenance score on Glama](https://glama.ai/mcp/servers/brechanbech/xojo-mcp/badges/score.svg)](https://glama.ai/mcp/servers/brechanbech/xojo-mcp)