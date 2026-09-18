#!/bin/sh
# Build the aarch64-apple-darwin release binary and publish it as an asset on
# the canonical Codeberg release. Codeberg's hosted Actions runners are Linux
# only, so the macOS build happens here, on an Apple Silicon Mac, rather than
# in CI.
#
# Usage:
#   scripts/release-binary.sh              # release the version in Cargo.toml
#   scripts/release-binary.sh v3.3.1       # release an explicit tag
#   scripts/release-binary.sh --dry-run    # build and package, upload nothing
#
# Needs CODEBERG_TOKEN (or FORGEJO_TOKEN) holding a token with repository write
# access, unless --dry-run is given. Re-running for the same tag is safe: the
# release is reused and assets of the same name are replaced.

set -e

TARGET="aarch64-apple-darwin"
DRY_RUN=0
TAG=""

usage() {
    sed -n '2,15p' "$0" | sed 's/^#\{1,\} \{0,1\}//'
}

for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=1 ;;
        -h|--help) usage; exit 0 ;;
        v*) TAG="$arg" ;;
        *)
            echo "Error: unrecognised argument: $arg" >&2
            echo >&2
            usage >&2
            exit 1
            ;;
    esac
done

cd "$(dirname "$0")/.."

version=$(sed -n '/^\[package\]/,/^\[/ s/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
if [ -z "$version" ]; then
    echo "Error: could not read the package version from Cargo.toml" >&2
    exit 1
fi

[ -n "$TAG" ] || TAG="v$version"

# ── Preflight ───────────────────────────────────────────────────────────────
# A downloadable binary is only useful if you can tell which source it came
# from, so refuse anything that is not exactly the tagged, committed tree.

if [ "$TAG" != "v$version" ]; then
    echo "Error: tag $TAG does not match the Cargo.toml version ($version)." >&2
    echo "       Bump Cargo.toml first, or pass the matching tag." >&2
    exit 1
fi

if ! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
    echo "Error: tag $TAG does not exist locally. Create it first:" >&2
    echo "       git tag -a $TAG -m 'xojo-mcp $version'" >&2
    exit 1
fi

if [ -n "$(git status --porcelain)" ]; then
    echo "Error: the working tree is dirty; commit or stash before releasing." >&2
    exit 1
fi

# The binary must be the one the tag describes. Comparing commits would be too
# strict -- docs and tooling land after a tag all the time -- so compare only
# the inputs that actually reach the binary: the sources, the manifest, the
# lockfile, and the usage guide that is compiled in via include_str!.
BUILD_INPUTS="src Cargo.toml Cargo.lock usage-guide.md"

drift=$(git diff --name-only "$TAG" HEAD -- $BUILD_INPUTS)
if [ -n "$drift" ]; then
    echo "Error: HEAD differs from $TAG in files that go into the binary:" >&2
    echo "$drift" | sed 's/^/         /' >&2
    echo "       Check out $TAG, or tag this commit instead." >&2
    exit 1
fi

if [ "$(git rev-parse HEAD)" != "$(git rev-parse "$TAG^{commit}")" ]; then
    echo "Note: HEAD is ahead of $TAG, but only in files outside the binary."
fi

# ── Build ───────────────────────────────────────────────────────────────────

echo "Building $TARGET (release)..."
cargo build --release --locked --target "$TARGET"

bin="target/$TARGET/release/xmcp"
if [ ! -f "$bin" ]; then
    echo "Error: expected binary not found at $bin" >&2
    exit 1
fi

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

pkg="xmcp-$version-$TARGET"
mkdir "$stage/$pkg"
cp "$bin" "$stage/$pkg/xmcp"
strip -x "$stage/$pkg/xmcp"

# The usage guide ships alongside: xmcp prefers a copy next to the executable
# and falls back to the one compiled in, so this lets a user edit it in place.
cp README.md LICENSE.md usage-guide.md "$stage/$pkg/"

# Never ship a binary for the wrong architecture, however we got here.
archs=$(lipo -archs "$stage/$pkg/xmcp")
if [ "$archs" != "arm64" ]; then
    echo "Error: built binary reports architecture '$archs', expected arm64." >&2
    exit 1
fi
echo "  stripped: $(du -h "$stage/$pkg/xmcp" | cut -f1 | tr -d ' ') ($archs)"

mkdir -p dist
tarball="dist/$pkg.tar.gz"
tar -C "$stage" -czf "$tarball" "$pkg"
(cd dist && shasum -a 256 "$pkg.tar.gz" > "$pkg.tar.gz.sha256")

echo "  → $tarball ($(du -h "$tarball" | cut -f1 | tr -d ' '))"
echo "  → $tarball.sha256"

if [ "$DRY_RUN" -eq 1 ]; then
    echo "Dry run: nothing uploaded."
    exit 0
fi

# ── Publish ─────────────────────────────────────────────────────────────────

TOKEN="${CODEBERG_TOKEN:-$FORGEJO_TOKEN}"
if [ -z "$TOKEN" ]; then
    echo "Error: set CODEBERG_TOKEN (or FORGEJO_TOKEN) to a token with" >&2
    echo "       repository write access on Codeberg." >&2
    exit 1
fi

# Derive owner/repo from the Codeberg remote rather than hardcoding a slug the
# repo has already changed once.
remote=$(git remote get-url origin)
slug=$(echo "$remote" | sed -e 's|^.*codeberg\.org[:/]||' -e 's|\.git$||')
case "$slug" in
    */*) ;;
    *)
        echo "Error: could not derive owner/repo from origin ($remote)." >&2
        exit 1
        ;;
esac

api="https://codeberg.org/api/v1/repos/$slug"
auth="Authorization: token $TOKEN"

# The annotated tag is the source of truth for release notes.
notes=$(git tag -l --format='%(contents)' "$TAG")

echo "Creating Codeberg release $TAG on $slug..."
release_id=$(curl -sS -X POST "$api/releases" \
    -H "$auth" \
    -H "Content-Type: application/json" \
    -d "$(jq -n --arg tag "$TAG" --arg name "xojo-mcp $version" --arg body "$notes" \
        '{tag_name: $tag, name: $name, body: $body}')" \
    | jq -r '.id // empty')

if [ -z "$release_id" ]; then
    # Most likely it already exists, which makes a re-run the normal case.
    release_id=$(curl -sS "$api/releases/tags/$TAG" -H "$auth" | jq -r '.id // empty')
    [ -n "$release_id" ] && echo "  release exists already, reusing it"
fi

if [ -z "$release_id" ]; then
    echo "Error: could not create or find release $TAG on $slug." >&2
    echo "       Check that the tag is pushed and the token has write access." >&2
    exit 1
fi

for file in "$tarball" "$tarball.sha256"; do
    name=$(basename "$file")

    # Forgejo keeps same-named assets side by side, so clear the old one first.
    existing=$(curl -sS "$api/releases/$release_id/assets" -H "$auth" \
        | jq -r --arg n "$name" '.[] | select(.name == $n) | .id')
    for id in $existing; do
        curl -sS -X DELETE "$api/releases/$release_id/assets/$id" -H "$auth" >/dev/null
        echo "  replaced existing $name"
    done

    curl -sS -X POST "$api/releases/$release_id/assets?name=$name" \
        -H "$auth" \
        -F "attachment=@$file;type=application/octet-stream" \
        | jq -r '"  uploaded " + .name'
done

echo "https://codeberg.org/$slug/releases/tag/$TAG"
