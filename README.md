# xojo-mcp

An MCP (Model Context Protocol) server that gives AI assistants direct control
over the Xojo IDE. Communicates via stdin/stdout JSON-RPC and forwards IDE
commands via a Unix domain socket to the running Xojo IDE process.

> **Note on the name.** This crate was previously published as `xmcp`. It was
> renamed to `xojo-mcp` to avoid confusion with X's (formerly Twitter)
> unrelated `xmcp` framework. The installed **binary is still `xmcp`**, so
> existing configs keep working.

> **macOS only.** xojo-mcp talks to the Xojo IDE over its macOS-specific Unix
> domain socket (`/tmp/XojoIDE`). Windows and Linux are not supported and there
> is no plan to add support — the underlying IDE IPC mechanism doesn't exist
> on those platforms.

## Attribution

This is a Rust port of [XMCP](https://github.com/o3jvind/XMCP) by
Øjvind Søgaard Andersen, originally written in Xojo. The original project
is licensed under the MIT License.

## Quick start

### 1. Build and install

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
3. Claude will automatically discover the 25 xmcp tools and the usage guide

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

Read-only mode disables the five tools that modify the project:

| Tool | What it would otherwise do |
| --- | --- |
| `set_code` | Overwrite the code of a method, property, or other item |
| `set_selected_text` | Replace the current text selection in the code editor |
| `create_project_item` | Add a new class, module, window, or other item |
| `revert_project` | Discard unsaved changes back to the last save |
| `save_project` | Write the project's current in-memory state to disk |

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
   the server what tools exist (`tools/list`), the five mutating tools are
   filtered out. The model never sees them, so it cannot choose to call
   something it does not know exists. This is what makes the mode effective in
   practice rather than merely defensive.
2. **Any call to a blocked tool is rejected.** If a request to one of the five
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

## Differences from the original

This is a drop-in replacement — it exposes the same 25 tools as the original,
with identical names and parameters (`analyze_project` and `debug_control` were
the last two ported, bringing it to full parity). Same IDE Communicator
Protocol v2 over the Unix domain socket, updated to MCP protocol version
`2025-11-25`.

Notable differences:

- **Binary name** is `xmcp`
- **Enforced read-only mode** — `--read-only` / `XMCP_READ_ONLY` removes and
  rejects the mutating tools at the server. The original has no built-in
  enforcement; it can only be asked, via the prompt, not to write. See
  [Read-only mode](#read-only-mode).
- **No Xojo license required** — builds with the standard Rust toolchain
- **usage-guide.md has a compiled-in fallback** — the original fails silently
  if the file is missing next to the binary; the Rust version embeds a copy
  at compile time so the MCP resource is always available. A file on disk
  still takes priority, so you can edit it without rebuilding.
- **CLI parsing** uses [clap](https://crates.io/crates/clap) rather than the
  original's custom OptionParser. The flags are the same.

## Tools

xmcp exposes 25 tools across four categories:

**IDE tools (19):** list_project_items, get_current_location, select_project_item,
get_code, set_code, get_selected_text, set_selected_text, build_project,
run_project, stop_project, create_project_item, run_ide_script, get_project_info,
revert_project, save_project, get_item_description, constant_value,
analyze_project, debug_control

**Documentation tools (3):** search_docs, lookup_class, list_doc_topics

**Debug tools (2):** get_debug_log, get_system_log

**Cost awareness (1):** estimate_request_cost

## License

MIT — see [LICENSE.md](LICENSE.md) for details.

## MCP registry

Ownership-verification token for the [MCP registry](https://registry.modelcontextprotocol.io)
(read from this crate's rendered README on crates.io):

> Registry ownership token: `mcp-name: io.github.brechanbech/xojo-mcp`
