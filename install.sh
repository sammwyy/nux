#!/usr/bin/env sh
# Installs the latest nux release for this machine's OS/arch.
#   curl -fsSL https://raw.githubusercontent.com/sammwyy/nux/main/install.sh | sh
set -eu

REPO="sammwyy/nux"
API_URL="https://api.github.com/repos/${REPO}/releases/latest"

detect_os() {
    case "$(uname -s)" in
        Linux) echo linux ;;
        *)
            echo "✗ no prebuilt binary for $(uname -s) — try: cargo install nux-term" >&2
            exit 1
            ;;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64 | amd64) echo x86_64 ;;
        aarch64 | arm64) echo arm64 ;;
        armv7l | armv6l) echo armv7 ;;
        *)
            echo "✗ no prebuilt binary for architecture $(uname -m)" >&2
            exit 1
            ;;
    esac
}

echo "detecting system..."
OS=$(detect_os)
ARCH=$(detect_arch)
ASSET="nux-${OS}-${ARCH}.tar.gz"
echo "  → ${OS}/${ARCH}"

echo "fetching latest release..."
RELEASE_JSON=$(curl -fsSL "$API_URL")
DOWNLOAD_URL=$(printf '%s' "$RELEASE_JSON" \
    | grep -o "\"browser_download_url\": *\"[^\"]*${ASSET}\"" \
    | sed -E 's/.*"(https:[^"]+)"/\1/' \
    | head -n1)
if [ -z "$DOWNLOAD_URL" ]; then
    echo "✗ no release asset named ${ASSET} was found for ${REPO}" >&2
    exit 1
fi

TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

echo "downloading ${ASSET}..."
curl -fsSL "$DOWNLOAD_URL" -o "$TMP_DIR/$ASSET"
tar xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR"

BIN_SRC=$(find "$TMP_DIR" -type f -name nux | head -n1)
if [ -z "$BIN_SRC" ]; then
    echo "✗ couldn't find the nux binary inside ${ASSET}" >&2
    exit 1
fi

INSTALL_DIR="${NUX_INSTALL_DIR:-$HOME/.local/bin}"
echo "installing to ${INSTALL_DIR}..."
mkdir -p "$INSTALL_DIR"
chmod +x "$BIN_SRC"
cp "$BIN_SRC" "$INSTALL_DIR/nux"

PATH_NOTE=""
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        case "${SHELL:-}" in
            */zsh) RC_FILE="$HOME/.zshrc" ;;
            */bash) RC_FILE="$HOME/.bashrc" ;;
            *) RC_FILE="$HOME/.profile" ;;
        esac
        LINE="export PATH=\"${INSTALL_DIR}:\$PATH\""
        if grep -qF "$LINE" "$RC_FILE" 2>/dev/null; then
            PATH_NOTE="added to PATH in ${RC_FILE}"
        elif printf '\n# added by nux install.sh\n%s\n' "$LINE" >> "$RC_FILE" 2>/dev/null; then
            PATH_NOTE="added to PATH in ${RC_FILE}"
        else
            PATH_NOTE="  ${LINE}"
        fi
        export PATH="${INSTALL_DIR}:${PATH}"
        ;;
esac

echo
echo "✓ $("$INSTALL_DIR/nux" --version) → ${INSTALL_DIR}"
[ -n "$PATH_NOTE" ] && printf '%s\n' "$PATH_NOTE"
