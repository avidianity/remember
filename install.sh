#!/bin/sh
# remember installer - downloads the prebuilt release binary for your platform
# and drops it on your PATH. Safe to pipe straight from the web:
#
#   sh -c "$(curl -fsSL https://raw.githubusercontent.com/avidianity/remember/main/install.sh)"
#
# Environment overrides:
#   REMEMBER_VERSION      install a specific tag (e.g. v0.1.0) instead of the latest
#   REMEMBER_INSTALL_DIR  install location (default: $HOME/.local/bin)

set -eu

REPO="avidianity/remember"
BIN="remember"
INSTALL_DIR="${REMEMBER_INSTALL_DIR:-$HOME/.local/bin}"

err()  { printf 'remember-install: error: %s\n' "$1" >&2; exit 1; }
info() { printf 'remember-install: %s\n' "$1" >&2; }

# --- pick a downloader -------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  dl_to() { curl -fsSL "$1" -o "$2"; }
  dl_out() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
  dl_to() { wget -qO "$2" "$1"; }
  dl_out() { wget -qO- "$1"; }
else
  err "this installer needs 'curl' or 'wget'"
fi

# --- detect platform ---------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux)  os_part="unknown-linux-gnu" ;;
  Darwin) os_part="apple-darwin" ;;
  *) err "unsupported OS '$os'. On Windows, download the .zip from https://github.com/$REPO/releases" ;;
esac

case "$arch" in
  x86_64 | amd64)  arch_part="x86_64" ;;
  arm64 | aarch64) arch_part="aarch64" ;;
  *) err "unsupported architecture '$arch'" ;;
esac

target="${arch_part}-${os_part}"

# --- resolve version ---------------------------------------------------------
version="${REMEMBER_VERSION:-}"
if [ -z "$version" ]; then
  info "Resolving latest release..."
  version="$(dl_out "https://api.github.com/repos/$REPO/releases/latest" \
    | grep '"tag_name"' | head -n 1 \
    | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
  [ -n "$version" ] || err "could not determine the latest release; set REMEMBER_VERSION to a tag like v0.1.0"
fi

asset="${BIN}-${target}.tar.gz"
# The release publishes the checksum as <bin>-<target>.sha256 (extension
# replaced, not appended), and its contents reference the .tar.gz archive.
checksum="${BIN}-${target}.sha256"
base_url="https://github.com/$REPO/releases/download/$version"

info "Installing $BIN $version ($target)"

# --- download, verify, extract ----------------------------------------------
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

dl_to "$base_url/$asset" "$tmp/$asset" || err "download failed: $base_url/$asset"

if dl_to "$base_url/$checksum" "$tmp/$checksum" 2>/dev/null; then
  (
    cd "$tmp"
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum -c "$checksum" >/dev/null 2>&1 || err "checksum verification failed"
      info "Checksum OK"
    elif command -v shasum >/dev/null 2>&1; then
      shasum -a 256 -c "$checksum" >/dev/null 2>&1 || err "checksum verification failed"
      info "Checksum OK"
    else
      info "warning: no sha256 tool found; skipping checksum verification"
    fi
  )
else
  info "warning: no checksum published for this asset; skipping verification"
fi

tar -xzf "$tmp/$asset" -C "$tmp" || err "failed to extract $asset"

binpath="$(find "$tmp" -type f -name "$BIN" | head -n 1)"
[ -n "$binpath" ] || err "could not find '$BIN' inside the downloaded archive"
chmod +x "$binpath"

# --- install -----------------------------------------------------------------
mkdir -p "$INSTALL_DIR"
mv "$binpath" "$INSTALL_DIR/$BIN"
info "Installed to $INSTALL_DIR/$BIN"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    info ""
    info "$INSTALL_DIR is not on your PATH. Add this to your shell profile:"
    info "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac

info ""
info "Done. Register it with your agents:"
info "  $BIN setup claude-code"
info "  $BIN setup codex"
info "  $BIN setup opencode"
