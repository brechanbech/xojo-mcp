# Releasing xojo-mcp

Maintainer notes. Not shipped in the crate (see `exclude` in `Cargo.toml`).

A version can't be changed once it's published. crates.io can yank a version but not delete
it, and a version number is never reused, so do the steps in this order.

## 1. Check the tree

```sh
cargo fmt --check
cargo clippy --all-targets
cargo test
```

CI runs the same three checks, so a crate that isn't `cargo fmt` clean fails there too.

If you published since the last version bump, run `cargo clean -p xojo-mcp` first.
`cargo publish` builds the packaged copy into the same `target/debug`, and until the version
changes cargo treats that build as up to date. The integration tests in
`tests/protocol_surface.rs` then run the published binary instead of the current source, while
the unit tests pass against the current code. If
`grep -c target/package target/debug/xmcp.d` prints anything but 0, the build is stale.

## 2. Bump the version

- **`Cargo.toml`**: `version`. `Cargo.lock` picks up the change on the next build.
- **`server.json`**: *both* `version` fields, the top-level one and the one under
  `packages[]`. Nothing checks this file: no script, no CI job and no test reads it. It stayed
  at 3.4.0 through the 3.4.1 and 3.5.0 releases, so the registry advertised a version two
  releases older than the one people could install.

## 3. Commit, tag, push

```sh
git tag -a vX.Y.Z -m 'xojo-mcp X.Y.Z'
git push --follow-tags
```

`origin` has two push URLs, so one push updates both Codeberg and the GitHub mirror. Wait for
CI to pass before you publish.

## 4. crates.io

```sh
cargo publish --dry-run
cargo publish
```

The dry run compiles *from the packaged copy*, so it catches a crate that depends on a file it
doesn't ship. Files listed in `exclude` are among the files it doesn't ship.

## 5. MCP registry

Do this **after** crates.io, not before. The registry checks namespace ownership by reading
`mcp-name` from this crate's *README as rendered on crates.io*, and `server.json` names a
cargo version that has to exist there already.

```sh
mcp-publisher validate      # checks server.json against the live registry
mcp-publisher login github  # interactive, for the io.github.brechanbech/* namespace
mcp-publisher publish
```

The GitHub login doesn't last from one release to the next. If `publish` returns 401 "token is
expired", log in again.

## 6. Prebuilt binary (Codeberg release)

On an Apple Silicon Mac, with the tag checked out or with HEAD differing from it only in files
that don't go into the binary:

```sh
scripts/package-binary.sh
```

The script builds `aarch64-apple-darwin`, signs it with the Developer ID certificate, notarizes
it, and writes the following to `dist/`:

- the tarball
- its `.sha256`
- its minisign `.minisig`

The script refuses to run if the tree is dirty, if the tag doesn't match `Cargo.toml`, or if the
signing identity or minisign key is missing. Read the script header for the overrides
(`XMCP_SIGN_IDENTITY`, `XMCP_NOTARY_PROFILE`, `XMCP_MINISIGN_KEY`).

Publish through the forgejo MCP server in write mode. Don't use `curl` with a token in the
shell:

1. `get_release` for the tag. If it returns 404, `create_release` it, with notes that include
   an Install section.
2. `delete_release_asset` for any asset with the same name already attached. Forgejo keeps
   same-named assets side by side instead of replacing them, so a re-run needs this step.
3. `upload_release_asset` for the tarball, the `.sha256` and the `.minisig`.

To correct published notes, use `edit_release`. Deleting and recreating the release deletes its
assets too.

## 7. GitHub mirror release

Pushing to the mirror copies tags but not releases. Glama's maintenance score reads releases
from GitHub, so create a release there with the notes only and no assets:

```sh
gh release create vX.Y.Z -R brechanbech/xojo-mcp --verify-tag --latest \
    --title "xojo-mcp X.Y.Z" --notes-file <notes>
```

Use the Codeberg notes, but replace the Install section with a link to the Codeberg release,
where the signed binaries are.

## 8. Glama

On the Glama admin page for the listing, **Sync** the repository and then **Build & Release**,
so the listing shows the new version. Glama's cached head can lag behind GitHub, so check that
it has picked up the release commit before building.
