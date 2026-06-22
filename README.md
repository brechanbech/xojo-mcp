# xmcp-rs

<!-- mcp-name: io.github.brechanbech/xmcp -->

An MCP (Model Context Protocol) server that gives AI assistants direct control
over the Xojo IDE. Communicates via stdin/stdout JSON-RPC and forwards IDE
commands via a Unix domain socket to the running Xojo IDE process.

> **macOS only.** xmcp-rs talks to the Xojo IDE over its macOS-specific Unix
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
git clone https://codeberg.org/brechanbech/xmcp-rs.git
cd xmcp-rs
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
3. Claude will automatically discover the 23 xmcp tools and the usage guide

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

## Requirements

- macOS (the Xojo IDE IPC socket is macOS-specific)
- Rust toolchain (`rustup` — https://rustup.rs)
- Xojo IDE must be running with a project open before using any tools

## Options

```
xmcp [OPTIONS]
```

- `-v`, `--verbose` — Enable verbose logging to stderr
- `-d`, `--docs-path <PATH>` — Path to Xojo documentation directory (auto-detected if omitted)
- `-V`, `--version` — Print version
- `-h`, `--help` — Print help

## Differences from the original

This is a drop-in replacement — all 22 original tools are preserved with
identical names and parameters, plus one new tool (`estimate_request_cost`)
for a total of 23. Same IDE Communicator Protocol v2 over the Unix domain
socket, updated to MCP protocol version `2025-11-25`.

Notable differences:

- **Binary name** is `xmcp`
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

MIT — see [LICENSE](LICENSE) for details.
