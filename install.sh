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
            echo "error: no prebuilt binary for $(uname -s) — only Linux is published." >&2
            echo "       try 'cargo install nux-term' instead." >&2
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
            echo "error: no prebuilt binary for architecture $(uname -m)." >&2
            exit 1
            ;;
    esac
}

OS=$(detect_os)
ARCH=$(detect_arch)
ASSET="nux-${OS}-${ARCH}.tar.gz"

echo "Detected ${OS}/${ARCH}; looking up the latest release..."

RELEASE_JSON=$(curl -fsSL "$API_URL")
DOWNLOAD_URL=$(printf '%s' "$RELEASE_JSON" \
    | grep -o "\"browser_download_url\": *\"[^\"]*${ASSET}\"" \
    | sed -E 's/.*"(https:[^"]+)"/\1/' \
    | head -n1)

if [ -z "$DOWNLOAD_URL" ]; then
    echo "error: no release asset named ${ASSET} was found in the latest release of ${REPO}." >&2
    exit 1
fi

TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

echo "Downloading ${DOWNLOAD_URL}"
curl -fsSL "$DOWNLOAD_URL" -o "$TMP_DIR/$ASSET"
tar xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR"

BIN_SRC=$(find "$TMP_DIR" -type f -name nux | head -n1)
if [ -z "$BIN_SRC" ]; then
    echo "error: couldn't find the nux binary inside ${ASSET}." >&2
    exit 1
fi

INSTALL_DIR="${NUX_INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$INSTALL_DIR"
chmod +x "$BIN_SRC"
cp "$BIN_SRC" "$INSTALL_DIR/nux"
echo "Installed nux to ${INSTALL_DIR}/nux"

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        case "${SHELL:-}" in
            */zsh) RC_FILE="$HOME/.zshrc" ;;
            */bash) RC_FILE="$HOME/.bashrc" ;;
            *) RC_FILE="$HOME/.profile" ;;
        esac
        LINE="export PATH=\"${INSTALL_DIR}:\$PATH\""
        if ! grep -qF "$LINE" "$RC_FILE" 2>/dev/null; then
            printf '\n# added by nux install.sh\n%s\n' "$LINE" >> "$RC_FILE"
        fi
        echo "Added ${INSTALL_DIR} to PATH in ${RC_FILE} — restart your shell, or run:"
        echo "  ${LINE}"
        ;;
esac

"$INSTALL_DIR/nux" --version
