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
info  "Zero-dependency Delta Lake observability CLI — written in Rust"
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
    if [ -n "${DELTALENS_INSTALL_DIR:-}" ]; then
        INSTALL_DIR="$DELTALENS_INSTALL_DIR"
    elif [ -w "/usr/local/bin" ] || sudo -n true 2>/dev/null; then
        INSTALL_DIR="/usr/local/bin"
        USE_SUDO=true
    else
        INSTALL_DIR="${HOME}/.local/bin"
        USE_SUDO=false
        mkdir -p "$INSTALL_DIR"

        # Add to PATH hint if not already there
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

    # Verify checksum if available
    if curl -fsSL "$sha_url" -o "${tmp}/archive.tar.gz.sha256" 2>/dev/null; then
        info "Verifying SHA256 checksum..."
        (cd "$tmp" && sed -i "s|${BINARY}-${VERSION}-${ASSET_SUFFIX%.tar.gz}.tar.gz|archive.tar.gz|g" archive.tar.gz.sha256 && sha256sum -c archive.tar.gz.sha256 --quiet) \
            && success "Checksum verified" \
            || warn "Checksum verification failed — continuing anyway (network issue?)"
    else
        warn "No checksum file found, skipping verification."
    fi

    # Extract
    tar -xzf "${tmp}/archive.tar.gz" -C "$tmp"

    # Find the binary (it's nested in a dir)
    local extracted_bin
    extracted_bin=$(find "$tmp" -type f -name "$BINARY" | head -1)
    [ -n "$extracted_bin" ] || error "Binary not found in archive. Please file an issue: ${GITHUB}/issues"

    # Install
    if [ "${USE_SUDO:-false}" = "true" ]; then
        sudo install -m 755 "$extracted_bin" "${INSTALL_DIR}/${BINARY}"
    else
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
