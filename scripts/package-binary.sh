#!/bin/sh
# Build the aarch64-apple-darwin release binary, sign and notarize it, and
# package it into dist/ ready to publish. Codeberg's hosted Actions runners are
# Linux only, so the macOS build happens here, on an Apple Silicon Mac, rather
# than in CI.
#
# Publishing is deliberately not this script's job. The release and its assets go
# up through the forgejo MCP server, which holds its own write token behind a
# time-boxed elevation — so no release token has to exist in a shell for this to
# read, and none is read.
#
# Usage:
#   scripts/package-binary.sh              # package the version in Cargo.toml
#   scripts/package-binary.sh v3.4.1       # package an explicit tag
#   scripts/package-binary.sh --unsigned   # skip signing and notarization
#
# Signing needs the Developer ID Application identity in the login keychain and
# a stored notarytool profile; override either with XMCP_SIGN_IDENTITY and
# XMCP_NOTARY_PROFILE. Notarization talks to Apple and takes a minute or two.

set -e

TARGET="aarch64-apple-darwin"
UNSIGNED=0
TAG=""

# Overridable so a renamed notarytool profile, or a move to another team, needs
# no edit here. The profile is stored once, and outlives this script:
#   xcrun notarytool store-credentials xojo-mcp \
#       --apple-id <apple-id> --team-id CGYN4PNM9S
SIGN_IDENTITY="${XMCP_SIGN_IDENTITY:-Developer ID Application: Metrakol, LLC (CGYN4PNM9S)}"
NOTARY_PROFILE="${XMCP_NOTARY_PROFILE:-xojo-mcp}"

usage() {
    sed -n '2,18p' "$0" | sed 's/^#\{1,\} \{0,1\}//'
}

for arg in "$@"; do
    case "$arg" in
        --unsigned) UNSIGNED=1 ;;
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

# Developer ID certificates expire, and a release build is a tedious way to
# discover that, so check the identity is usable before building anything.
if [ "$UNSIGNED" -eq 0 ]; then
    if ! security find-identity -v -p codesigning | grep -qF "$SIGN_IDENTITY"; then
        echo "Error: signing identity not in the keychain:" >&2
        echo "         $SIGN_IDENTITY" >&2
        echo "       Check 'security find-identity -v -p codesigning', set" >&2
        echo "       XMCP_SIGN_IDENTITY, or pass --unsigned." >&2
        exit 1
    fi
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

# ── Sign and notarize ───────────────────────────────────────────────────────
# Signing comes after strip, because stripping rewrites the Mach-O and would
# invalidate a signature applied before it. --timestamp is what keeps the
# signature valid once the certificate expires; --options runtime (hardened
# runtime) is a precondition for notarization.

if [ "$UNSIGNED" -eq 1 ]; then
    echo "Skipping signing and notarization (--unsigned)."
else
    echo "Signing as $SIGN_IDENTITY..."
    codesign --force --timestamp --options runtime \
        --sign "$SIGN_IDENTITY" "$stage/$pkg/xmcp"
    codesign --verify --strict "$stage/$pkg/xmcp"

    # notarytool accepts .zip, .pkg and .dmg but never .tar.gz, so submit a
    # throwaway ditto zip of the same staged directory. Apple documents ditto
    # as the archiver to use here; other zip tools can mangle the Mach-O.
    echo "Notarizing (this talks to Apple and takes a minute or two)..."
    ditto -c -k --keepParent "$stage/$pkg" "$stage/notarize.zip"

    log="$stage/notarytool.out"
    set +e
    xcrun notarytool submit "$stage/notarize.zip" \
        --keychain-profile "$NOTARY_PROFILE" --wait >"$log" 2>&1
    rc=$?
    set -e
    sed 's/^/  /' "$log"

    # Trust the reported status rather than the exit code alone: a submission
    # that comes back Invalid is a failure even where notarytool exits 0.
    if [ $rc -ne 0 ] || ! grep -q "status: Accepted" "$log"; then
        submission=$(sed -n 's/^ *id: *\([0-9a-f-][0-9a-f-]*\).*/\1/p' "$log" | head -1)
        echo "Error: notarization did not succeed." >&2
        if [ -n "$submission" ]; then
            echo "       For the rejection details run:" >&2
            echo "       xcrun notarytool log $submission \\" >&2
            echo "           --keychain-profile $NOTARY_PROFILE" >&2
        fi
        exit 1
    fi
    rm "$stage/notarize.zip" "$log"

    # A ticket cannot be stapled to a bare executable -- stapler handles only
    # .app, .dmg and .pkg -- so a user's Mac resolves this one online on first
    # run. spctl is no help in checking that: it answers "does not seem to be
    # an app" for any plain executable whatever its notarization status. The
    # "notarized" code requirement is the check that does work, and it makes
    # the same online lookup the user's Mac will make.
    if codesign -vvv -R="notarized" --check-notarization "$stage/$pkg/xmcp" \
        >/dev/null 2>&1; then
        echo "  notarized: ticket resolves, Gatekeeper will accept it"
    else
        echo "Error: signed, notarized as Accepted, but the ticket does not" >&2
        echo "       resolve yet -- usually propagation lag. Re-check with:" >&2
        echo "       codesign -vvv -R=\"notarized\" --check-notarization \\" >&2
        echo "           <binary>" >&2
        echo "       then re-run; refusing to package an unverifiable build." >&2
        exit 1
    fi
fi

mkdir -p dist
tarball="dist/$pkg.tar.gz"
tar -C "$stage" -czf "$tarball" "$pkg"
(cd dist && shasum -a 256 "$pkg.tar.gz" > "$pkg.tar.gz.sha256")

echo "  → $tarball ($(du -h "$tarball" | cut -f1 | tr -d ' '))"
echo "  → $tarball.sha256"

# ── Where to take it from here ──────────────────────────────────────────────
# Publishing goes through the forgejo MCP server, so print the calls rather than
# make them. Forgejo keeps same-named assets side by side instead of replacing,
# which makes the delete step load-bearing on any re-run, not a tidy-up.

slug=$(git remote get-url origin | sed -e 's|^.*codeberg\.org[:/]||' -e 's|\.git$||')

echo
echo "Ready to publish. Through the forgejo MCP server, in write mode:"
echo "  get_release           $slug $TAG            → the release id"
echo "  delete_release_asset  any same-named asset already attached"
echo "  upload_release_asset  $tarball"
echo "  upload_release_asset  $tarball.sha256"
