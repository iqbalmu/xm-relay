#!/usr/bin/env sh
# xm-relay installer
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/iqbalmu/xm-relay/main/install.sh | sh
#
# Environment variables:
#   XM_RELAY_INSTALL_DIR  Target directory for the binary (default: ~/.local/bin)
#   XM_RELAY_VERSION      Specific release tag to install (default: latest)
#   XM_RELAY_REPO         GitHub owner/repo (default: iqbalmu/xm-relay)

set -eu

REPO="${XM_RELAY_REPO:-iqbalmu/xm-relay}"
INSTALL_DIR="${XM_RELAY_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${XM_RELAY_VERSION:-latest}"
BIN_NAME="xmrelay"

say() {
    printf '[xm-relay] %s\n' "$*" >&2
}

err() {
    say "error: $*"
    exit 1
}

need_cmd() {
    command -v "$1" >/dev/null 2>&1
}

detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux) os_part="unknown-linux-gnu" ;;
        Darwin) os_part="apple-darwin" ;;
        *) err "unsupported operating system: $os (try: cargo install --git https://github.com/$REPO.git)" ;;
    esac

    case "$arch" in
        x86_64|amd64) arch_part="x86_64" ;;
        aarch64|arm64) arch_part="aarch64" ;;
        *) err "unsupported CPU architecture: $arch (try: cargo install --git https://github.com/$REPO.git)" ;;
    esac

    printf '%s-%s' "$arch_part" "$os_part"
}

verify_sha256() {
    archive="$1"
    sum_file="$2"
    expected="$(awk '{print $1}' "$sum_file")"
    if [ -z "$expected" ]; then
        err "empty checksum file"
    fi
    if need_cmd sha256sum; then
        actual="$(sha256sum "$archive" | awk '{print $1}')"
    elif need_cmd shasum; then
        actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
    else
        err "neither sha256sum nor shasum found to verify download integrity"
    fi
    if [ "$actual" != "$expected" ]; then
        err "SHA-256 mismatch! expected $expected, got $actual"
    fi
    say "SHA-256 checksum verified ($actual)"
}

fallback_cargo() {
    if need_cmd cargo; then
        say "No prebuilt release binary found; falling back to building from source with cargo..."
        cargo install --git "https://github.com/$REPO.git" --locked xm-relay
        exit 0
    fi
    err "prebuilt release binary not found for this platform/version and 'cargo' is not installed"
}

main() {
    need_cmd curl || err "'curl' is required"
    need_cmd tar || err "'tar' is required"

    target="$(detect_target)"
    asset="${BIN_NAME}-${target}.tar.gz"

    if [ "$VERSION" = "latest" ]; then
        base_url="https://github.com/${REPO}/releases/latest/download"
    else
        base_url="https://github.com/${REPO}/releases/download/${VERSION}"
    fi

    tmpdir="$(mktemp -d 2>/dev/null || mktemp -d -t xm-relay)"
    trap 'rm -rf "$tmpdir"' EXIT INT TERM

    say "Downloading ${asset} (${VERSION}) from ${REPO}..."
    if ! curl -fsSL "${base_url}/${asset}" -o "${tmpdir}/${asset}" 2>/dev/null; then
        fallback_cargo
    fi

    if ! curl -fsSL "${base_url}/${asset}.sha256" -o "${tmpdir}/${asset}.sha256" 2>/dev/null; then
        err "failed to download checksum file ${asset}.sha256"
    fi

    verify_sha256 "${tmpdir}/${asset}" "${tmpdir}/${asset}.sha256"

    tar -xzf "${tmpdir}/${asset}" -C "$tmpdir"
    [ -f "${tmpdir}/${BIN_NAME}" ] || err "archive did not contain '${BIN_NAME}' binary"

    mkdir -p "$INSTALL_DIR"
    mv "${tmpdir}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
    chmod +x "${INSTALL_DIR}/${BIN_NAME}"

    say "Installed ${BIN_NAME} to ${INSTALL_DIR}/${BIN_NAME}"
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) ;;
        *) say "Note: ${INSTALL_DIR} is not in your PATH. Add 'export PATH=\"${INSTALL_DIR}:\$PATH\"' to your shell profile." ;;
    esac
    "${INSTALL_DIR}/${BIN_NAME}" --version || true
}

main "$@"
