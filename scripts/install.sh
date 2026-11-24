#!/usr/bin/env bash
# ============================================================================
#  deltalens — Unix/Linux/macOS installer
#  Usage:  curl -fsSL https://raw.githubusercontent.com/sandy-sachin7/datalens/main/scripts/install.sh | bash
# ============================================================================
set -euo pipefail

REPO="sandy-sachin7/datalens"
BINARY="deltalens"
GITHUB="https://github.com/${REPO}"
API="https://api.github.com/repos/${REPO}"

# ── Colours ─────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'

info()    { printf "${CYAN}ℹ${NC}  %s\n" "$*"; }
success() { printf "${GREEN}✔${NC}  %s\n" "$*"; }
warn()    { printf "${YELLOW}⚠${NC}  %s\n" "$*"; }
error()   { printf "${RED}✘  %s${NC}\n" "$*" >&2; exit 1; }
bold()    { printf "${BOLD}%s${NC}\n" "$*"; }

# ── Banner ───────────────────────────────────────────────────────────────────
printf "\n"
bold "  ██████╗ ███████╗██╗  ████████╗ █████╗ ██╗     ███████╗███╗   ██╗███████╗"
bold "  ██╔══██╗██╔════╝██║  ╚══██╔══╝██╔══██╗██║     ██╔════╝████╗  ██║██╔════╝"
bold "  ██║  ██║█████╗  ██║     ██║   ███████║██║     █████╗  ██╔██╗ ██║███████╗"
bold "  ██║  ██║██╔══╝  ██║     ██║   ██╔══██║██║     ██╔══╝  ██║╚██╗██║╚════██║"
bold "  ██████╔╝███████╗███████╗██║   ██║  ██║███████╗███████╗██║ ╚████║███████║"
bold "  ╚═════╝ ╚══════╝╚══════╝╚═╝   ╚═╝  ╚═╝╚══════╝╚══════╝╚═╝  ╚═══╝╚══════╝"
printf "\n"
info  "Standalone CLI to inspect Delta Lake tables without Spark, JVM, or Python"
printf "\n"

# ── Detect OS & Architecture ─────────────────────────────────────────────────
detect_platform() {
    local os arch

    os=$(uname -s)
    arch=$(uname -m)

    case "$os" in
        Linux)  os="linux"  ;;
        Darwin) os="macos"  ;;
        *)      error "Unsupported OS: ${os}. Please build from source: ${GITHUB}" ;;
    esac

    case "$arch" in
        x86_64|amd64)   arch="x86_64"  ;;
        aarch64|arm64)  arch="aarch64" ;;
        *)              error "Unsupported architecture: ${arch}. Please build from source: ${GITHUB}" ;;
    esac

    PLATFORM="${arch}-${os}"
    ASSET_SUFFIX="${arch}-${os}.tar.gz"
    info "Detected platform: ${BOLD}${PLATFORM}${NC}"
}

# ── Resolve install directory ────────────────────────────────────────────────
resolve_install_dir() {
    USE_SUDO=false

    if [ -n "${DELTALENS_INSTALL_DIR:-}" ]; then
        INSTALL_DIR="$DELTALENS_INSTALL_DIR"
        mkdir -p "$INSTALL_DIR" 2>/dev/null || true
        if [ ! -w "$INSTALL_DIR" ]; then
            USE_SUDO=true
        fi
    elif [ -w "/usr/local/bin" ]; then
        INSTALL_DIR="/usr/local/bin"
        USE_SUDO=false
    elif sudo -n true 2>/dev/null; then
        INSTALL_DIR="/usr/local/bin"
        USE_SUDO=true
    else
        INSTALL_DIR="${HOME}/.local/bin"
        USE_SUDO=false
        mkdir -p "$INSTALL_DIR"

        if [[ ":$PATH:" != *":${INSTALL_DIR}:"* ]]; then
            warn "${INSTALL_DIR} is not in your PATH."
            warn "Add this to your shell profile (~/.bashrc / ~/.zshrc):"
            printf "\n    ${BOLD}export PATH=\"\$HOME/.local/bin:\$PATH\"${NC}\n\n"
        fi
    fi
    info "Install directory: ${BOLD}${INSTALL_DIR}${NC}"
}

# ── Fetch latest version tag ─────────────────────────────────────────────────
fetch_latest_version() {
    if [ -n "${DELTALENS_VERSION:-}" ]; then
        VERSION="$DELTALENS_VERSION"
    else
        info "Fetching latest release..."
        VERSION=$(curl -fsSL "${API}/releases/latest" \
            | grep '"tag_name"' \
            | sed 's/.*"tag_name": *"//; s/".*//')
        [ -n "$VERSION" ] || error "Could not determine latest version. Check ${GITHUB}/releases"
    fi
    info "Version: ${BOLD}${VERSION}${NC}"
}

# ── Check dependencies ───────────────────────────────────────────────────────
check_deps() {
    for cmd in curl tar; do
        command -v "$cmd" >/dev/null 2>&1 || error "Required tool not found: ${cmd}"
    done
    if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
        error "Required SHA-256 tool not found: install 'sha256sum' or 'shasum'"
    fi
}

compute_sha256() {
    local file="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" | awk '{print tolower($1)}'
    else
        shasum -a 256 "$file" | awk '{print tolower($1)}'
    fi
}

# ── Download & verify ────────────────────────────────────────────────────────
download_binary() {
    local url="${GITHUB}/releases/download/${VERSION}/${BINARY}-${VERSION}-${ASSET_SUFFIX}"
    local sha_url="${url}.sha256"
    local tmp
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT

    info "Downloading ${BOLD}${BINARY} ${VERSION}${NC} for ${BOLD}${PLATFORM}${NC}..."
    curl -fSL --progress-bar "$url" -o "${tmp}/archive.tar.gz" \
        || error "Download failed.\n  URL: ${url}\n  Check ${GITHUB}/releases for available assets."

    # Download and strictly enforce SHA-256 checksum verification
    info "Verifying SHA-256 checksum..."
    curl -fsSL "$sha_url" -o "${tmp}/archive.tar.gz.sha256" \
        || error "Failed to download SHA-256 checksum file (${sha_url}). Aborting installation."

    local expected actual
    expected=$(awk '{print tolower($1); exit}' "${tmp}/archive.tar.gz.sha256")
    [ -n "$expected" ] || error "Checksum file is empty or malformed. Aborting installation."
    actual=$(compute_sha256 "${tmp}/archive.tar.gz")

    if [ "$expected" != "$actual" ]; then
        error "SHA-256 checksum mismatch! Expected: ${expected}, Got: ${actual}. Aborting installation."
    fi
    success "SHA-256 checksum verified"

    # Extract
    tar -xzf "${tmp}/archive.tar.gz" -C "$tmp"

    local extracted_bin
    extracted_bin=$(find "$tmp" -type f -name "$BINARY" | head -1)
    [ -n "$extracted_bin" ] || error "Binary not found in archive. Please file an issue: ${GITHUB}/issues"

    # Install (only escalate with sudo if target dir is not writable)
    if [ "${USE_SUDO}" = "true" ]; then
        sudo install -d -m 755 "$INSTALL_DIR"
        sudo install -m 755 "$extracted_bin" "${INSTALL_DIR}/${BINARY}"
    else
        install -d -m 755 "$INSTALL_DIR"
        install -m 755 "$extracted_bin" "${INSTALL_DIR}/${BINARY}"
    fi
}

# ── Verify installation ───────────────────────────────────────────────────────
verify_install() {
    if command -v "${BINARY}" >/dev/null 2>&1; then
        local installed_version
        installed_version=$("${BINARY}" --version 2>&1 | head -1)
        success "Installed: ${BOLD}${installed_version}${NC}"
    elif "${INSTALL_DIR}/${BINARY}" --version >/dev/null 2>&1; then
        local installed_version
        installed_version=$("${INSTALL_DIR}/${BINARY}" --version 2>&1 | head -1)
        success "Installed to ${INSTALL_DIR}/${BINARY}: ${BOLD}${installed_version}${NC}"
    else
        warn "Binary installed but could not be verified. Try: ${INSTALL_DIR}/${BINARY} --version"
    fi
}

# ── Main ──────────────────────────────────────────────────────────────────────
main() {
    check_deps
    detect_platform
    resolve_install_dir
    fetch_latest_version
    download_binary
    verify_install

    printf "\n"
    success "${BOLD}deltalens is ready!${NC}"
    printf "\n"
    printf "  Quick start:\n"
    printf "    ${BOLD}${BINARY} inspect /path/to/delta/table${NC}\n"
    printf "    ${BOLD}${BINARY} --help${NC}\n"
    printf "\n"
    printf "  Docs & source: ${BLUE}${GITHUB}${NC}\n"
    printf "\n"
}

main "$@"
