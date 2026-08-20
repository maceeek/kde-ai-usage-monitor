#!/usr/bin/env bash
#
# Install the monitor into the current user's home — no root, no package
# manager. On Arch and CachyOS prefer the PKGBUILD in packaging/arch instead;
# this script is the fallback for every other distribution.
#
#   ./packaging/install.sh            # build, install binary + applet
#   ./packaging/install.sh --uninstall

set -euo pipefail

APPLET_ID="com.github.maceeek.aiusagemonitor"
BIN_NAME="kde-ai-usage-monitor"
PREFIX="${PREFIX:-${HOME}/.local}"
BIN_DIR="${PREFIX}/bin"
APPLET_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}/plasma/plasmoids/${APPLET_ID}"
REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

log() { printf '\033[1;34m==>\033[0m %s\n' "$1"; }
die() { printf '\033[1;31m==> error:\033[0m %s\n' "$1" >&2; exit 1; }

uninstall() {
    log "Removing ${BIN_DIR}/${BIN_NAME}"
    rm -f "${BIN_DIR}/${BIN_NAME}"
    log "Removing ${APPLET_DIR}"
    rm -rf "${APPLET_DIR}"
    log "Done. Remove the widget from your panel if it is still there."
    exit 0
}

[[ "${1:-}" == "--uninstall" ]] && uninstall

command -v cargo >/dev/null || die "cargo is required to build the backend (pacman -S rust, or rustup)"

log "Building the backend"
cargo build --release --manifest-path "${REPO_DIR}/Cargo.toml"

log "Installing the backend into ${BIN_DIR}"
install -Dm755 "${REPO_DIR}/target/release/${BIN_NAME}" "${BIN_DIR}/${BIN_NAME}"

log "Installing the Plasma applet into ${APPLET_DIR}"
rm -rf "${APPLET_DIR}"
mkdir -p "${APPLET_DIR}"
cp -r "${REPO_DIR}/plasmoid/package/." "${APPLET_DIR}/"

# kpackagetool6 registers the applet with a running Plasma session; a plain file
# copy is enough after the next restart, so a missing tool is not fatal.
if command -v kpackagetool6 >/dev/null; then
    log "Registering the applet with Plasma"
    kpackagetool6 --type Plasma/Applet --upgrade "${APPLET_DIR}" >/dev/null 2>&1 ||
        kpackagetool6 --type Plasma/Applet --install "${APPLET_DIR}" >/dev/null 2>&1 || true
fi

case ":${PATH}:" in
    *":${BIN_DIR}:"*) ;;
    *) log "Note: ${BIN_DIR} is not on your PATH — set the applet's 'Backend command' to ${BIN_DIR}/${BIN_NAME}" ;;
esac

log "Installed. Add 'AI Usage Monitor' to your panel:"
log "  right-click the panel → Add Widgets… → search for 'AI Usage'"
log "If the widget does not show up yet, restart Plasma with: kquitapp6 plasmashell && kstart plasmashell"
