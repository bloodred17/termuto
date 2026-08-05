#!/bin/sh
# Install termuto from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/bloodred17/termuto/main/install.sh | sh
#
# Environment:
#   TERMUTO_VERSION   tag to install, e.g. v0.1.0 (default: the latest release)
#   TERMUTO_BIN_DIR   where the binary lands (default: ~/.local/bin)
set -eu

REPO="bloodred17/termuto"
BIN_DIR="${TERMUTO_BIN_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || err "this installer needs '$1' on PATH"; }

need uname
need mktemp
need tar

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1" -o "$2"; }
  fetch_stdout() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO "$2" "$1"; }
  fetch_stdout() { wget -qO - "$1"; }
else
  err "this installer needs either curl or wget on PATH"
fi

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os" in
    Linux) os_part="unknown-linux-gnu" ;;
    Darwin) os_part="apple-darwin" ;;
    *) err "unsupported operating system: $os (termuto ships Linux and macOS builds)" ;;
  esac
  case "$arch" in
    x86_64 | amd64) arch_part="x86_64" ;;
    aarch64 | arm64) arch_part="aarch64" ;;
    *) err "unsupported architecture: $arch" ;;
  esac
  printf '%s-%s' "$arch_part" "$os_part"
}

latest_tag() {
  # Read the tag out of the redirect that /releases/latest issues, so the
  # installer does not need jq or a GitHub API token.
  if command -v curl >/dev/null 2>&1; then
    url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
      "https://github.com/$REPO/releases/latest")"
  else
    url="$(wget -qS --max-redirect=10 -O /dev/null \
      "https://github.com/$REPO/releases/latest" 2>&1 \
      | awk '/^  Location: /{ print $2 }' | tail -1)"
  fi
  tag="${url##*/}"
  [ -n "$tag" ] && [ "$tag" != "latest" ] || err "could not determine the latest release of $REPO"
  printf '%s' "$tag"
}

verify_checksum() {
  # archive, expected sums file, archive basename
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$1" | cut -d' ' -f1)"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$1" | cut -d' ' -f1)"
  else
    say "warning: no sha256sum or shasum found, skipping checksum verification"
    return 0
  fi
  expected="$(awk -v f="$3" '$2 == f || $2 == "*" f { print $1 }' "$2" | head -1)"
  [ -n "$expected" ] || err "no checksum for $3 in SHA256SUMS"
  [ "$actual" = "$expected" ] || err "checksum mismatch for $3 (expected $expected, got $actual)"
}

target="$(detect_target)"
tag="${TERMUTO_VERSION:-$(latest_tag)}"
version="${tag#v}"
archive="termuto-$version-$target.tar.gz"
base="https://github.com/$REPO/releases/download/$tag"

say "Installing termuto $tag ($target)"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

fetch "$base/$archive" "$tmp/$archive" \
  || err "no build for $target in release $tag (looked for $archive)"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" \
  || err "could not download SHA256SUMS for $tag"
verify_checksum "$tmp/$archive" "$tmp/SHA256SUMS" "$archive"

tar -C "$tmp" -xzf "$tmp/$archive"
mkdir -p "$BIN_DIR"
install -m 755 "$tmp/termuto-$version-$target/termuto" "$BIN_DIR/termuto" 2>/dev/null \
  || { cp "$tmp/termuto-$version-$target/termuto" "$BIN_DIR/termuto" && chmod 755 "$BIN_DIR/termuto"; }

say "Installed $BIN_DIR/termuto"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    say ""
    say "$BIN_DIR is not on your PATH. Add it, for example:"
    say "  echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.bashrc"
    ;;
esac

command -v mpv >/dev/null 2>&1 || {
  say ""
  say "Note: playback shells out to mpv, which was not found on PATH."
  say "Install it (https://mpv.io) or pass --player to use another one."
}

say ""
say "Run 'termuto' for the terminal UI, or 'termuto --help' for the CLI."
