#!/bin/sh
# remember installer - downloads the prebuilt release binary for your platform
# and drops it on your PATH. Safe to pipe straight from the web:
#
#   sh -c "$(curl -fsSL https://raw.githubusercontent.com/avidianity/remember/main/install.sh)"
#
# Environment overrides:
#   REMEMBER_VERSION       install a specific tag (e.g. v0.1.0) instead of the latest
#   REMEMBER_INSTALL_DIR   install location (default: $HOME/.local/bin)
#   REMEMBER_RELEASES_URL  where releases are published (default: GitHub)
#
# `remember update` runs this same script with REMEMBER_CURRENT_VERSION set,
# which skips the install when no newer release exists (REMEMBER_FORCE=1
# reinstalls anyway).

set -eu

REPO="avidianity/remember"
BIN="remember"
INSTALL_DIR="${REMEMBER_INSTALL_DIR:-$HOME/.local/bin}"
RELEASES="${REMEMBER_RELEASES_URL:-https://github.com/$REPO/releases}"
CURRENT="${REMEMBER_CURRENT_VERSION:-}"

err()  { printf 'remember-install: error: %s\n' "$1" >&2; exit 1; }
info() { printf 'remember-install: %s\n' "$1" >&2; }

# --- pick a downloader -------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  dl_to() { curl -fsSL "$1" -o "$2"; }
  final_url() { curl -fsSLI -o /dev/null -w '%{url_effective}' "$1"; }
elif command -v wget >/dev/null 2>&1; then
  dl_to() { wget -qO "$2" "$1"; }
  final_url() {
    wget -S --spider "$1" 2>&1 | sed -n 's/^ *Location: \([^ ]*\).*/\1/p' | tail -n 1
  }
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
  # releases/latest redirects to releases/tag/<version>; unlike the GitHub
  # API it has no hourly rate limit.
  version="$(final_url "$RELEASES/latest")" || version=""
  version="${version##*/}"
  case "$version" in
    v[0-9]*) ;;
    *) err "could not determine the latest release; set REMEMBER_VERSION to a tag like v0.1.0" ;;
  esac
fi

# Succeeds when version $1 is newer than $2 (both vX.Y.Z or X.Y.Z).
newer() {
  old_ifs="$IFS"
  IFS=.
  # shellcheck disable=SC2086
  set -- ${1#v} ${2#v}
  IFS="$old_ifs"
  [ "$#" -eq 6 ] || return 0
  for part in "$@"; do
    case "$part" in *[!0-9]* | "") return 0 ;; esac
  done
  [ "$1" -ne "$4" ] && { [ "$1" -gt "$4" ]; return; }
  [ "$2" -ne "$5" ] && { [ "$2" -gt "$5" ]; return; }
  [ "$3" -gt "$6" ]
}

if [ -n "$CURRENT" ] && [ "${REMEMBER_FORCE:-}" != 1 ] && ! newer "$version" "$CURRENT"; then
  info "$BIN v${CURRENT#v} is up to date (latest release: $version)"
  exit 0
fi

asset="${BIN}-${target}.tar.gz"
# The release publishes the checksum as <bin>-<target>.sha256 (extension
# replaced, not appended), and its contents reference the .tar.gz archive.
checksum="${BIN}-${target}.sha256"
base_url="$RELEASES/download/$version"

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
[ -w "$INSTALL_DIR" ] || err "$INSTALL_DIR is not writable; re-run with permission to write there"
# Stage next to the target, then rename over it: a rename is atomic and
# leaves agents running the old binary untouched, where a copy across
# filesystems would write into the running file.
staged="$INSTALL_DIR/.$BIN.$$"
cp "$binpath" "$staged" || err "could not write to $INSTALL_DIR"
chmod 755 "$staged"
mv -f "$staged" "$INSTALL_DIR/$BIN" || { rm -f "$staged"; err "could not replace $INSTALL_DIR/$BIN"; }
info "Installed to $INSTALL_DIR/$BIN"

if [ -n "$CURRENT" ]; then
  if [ "v${CURRENT#v}" = "$version" ]; then
    info "Reinstalled $BIN $version."
  else
    info "Updated $BIN v${CURRENT#v} to $version."
  fi
  info "Running agents keep the old version until they restart."
  exit 0
fi

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
