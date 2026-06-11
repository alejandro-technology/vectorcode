#!/usr/bin/env bash
set -euo pipefail

# VectorCode installer for macOS and Linux
# Usage: curl -fsSL https://raw.githubusercontent.com/your-org/vectorcode/main/install.sh | bash

REPO="your-org/vectorcode"
BINARY_NAME="vectorcode"
INSTALL_DIR="/usr/local/bin"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info() {
    echo -e "${GREEN}✓${NC} $1"
}

warn() {
    echo -e "${YELLOW}⚠${NC} $1"
}

error() {
    echo -e "${RED}✗${NC} $1"
    exit 1
}

detect_os() {
    local os
    os="$(uname -s)"
    case "$os" in
        Linux*)  echo "linux" ;;
        Darwin*) echo "darwin" ;;
        *)       error "Unsupported OS: $os. VectorCode supports macOS and Linux." ;;
    esac
}

detect_arch() {
    local arch
    arch="$(uname -m)"
    case "$arch" in
        x86_64|amd64)   echo "x86_64" ;;
        aarch64|arm64)  echo "aarch64" ;;
        *)              error "Unsupported architecture: $arch" ;;
    esac
}

check_dependencies() {
    local missing=()

    if ! command -v curl &>/dev/null; then
        missing+=("curl")
    fi

    if [[ ${#missing[@]} -gt 0 ]]; then
        error "Missing dependencies: ${missing[*]}. Please install them first."
    fi
}

install_from_source() {
    info "Building from source..."

    if ! command -v cargo &>/dev/null; then
        error "Rust/Cargo not found. Install from https://rustup.rs or use the binary release."
    fi

    local tmpdir
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT

    info "Cloning repository..."
    git clone --depth 1 "https://github.com/${REPO}.git" "$tmpdir/vectorcode" 2>/dev/null || \
        error "Failed to clone repository. Check the REPO variable or your network."

    info "Building release binary (this may take a few minutes)..."
    cargo build --release --manifest-path "$tmpdir/vectorcode/Cargo.toml" 2>/dev/null || \
        error "Build failed. Check the error messages above."

    install_binary "$tmpdir/vectorcode/target/release/$BINARY_NAME"
}

install_binary() {
    local binary_path="$1"

    if [[ ! -f "$binary_path" ]]; then
        error "Binary not found at: $binary_path"
    fi

    # Check if install dir is writable
    if [[ -w "$INSTALL_DIR" ]]; then
        cp "$binary_path" "$INSTALL_DIR/$BINARY_NAME"
        chmod +x "$INSTALL_DIR/$BINARY_NAME"
    else
        info "Installing to $INSTALL_DIR requires sudo"
        sudo cp "$binary_path" "$INSTALL_DIR/$BINARY_NAME"
        sudo chmod +x "$INSTALL_DIR/$BINARY_NAME"
    fi

    info "Installed $BINARY_NAME to $INSTALL_DIR/$BINARY_NAME"
}

download_release() {
    local os="$1"
    local arch="$2"
    local version="${3:-latest}"

    local download_url
    if [[ "$version" == "latest" ]]; then
        download_url="https://github.com/${REPO}/releases/latest/download/${BINARY_NAME}-${os}-${arch}"
    else
        download_url="https://github.com/${REPO}/releases/download/${version}/${BINARY_NAME}-${os}-${arch}"
    fi

    local tmpdir
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT

    info "Downloading $BINARY_NAME for $os/$arch..."
    if curl -fsSL -o "$tmpdir/$BINARY_NAME" "$download_url" 2>/dev/null; then
        chmod +x "$tmpdir/$BINARY_NAME"
        install_binary "$tmpdir/$BINARY_NAME"
    else
        warn "Binary release not available for $os/$arch. Falling back to source build."
        install_from_source
    fi
}

main() {
    echo ""
    echo "  VectorCode Installer"
    echo "  ════════════════════"
    echo ""

    check_dependencies

    local os arch
    os="$(detect_os)"
    arch="$(detect_arch)"

    info "Detected: $os/$arch"

    # Parse arguments
    local method="auto"
    for arg in "$@"; do
        case "$arg" in
            --source)     method="source" ;;
            --binary)     method="binary" ;;
            --version=*)  version="${arg#--version=}" ;;
            --help|-h)
                echo "Usage: install.sh [--source|--binary] [--version=TAG]"
                echo ""
                echo "Options:"
                echo "  --source    Build from source (requires Rust/Cargo)"
                echo "  --binary    Download pre-built binary (default: auto)"
                echo "  --version   Specific version tag (default: latest)"
                echo "  --help      Show this help"
                exit 0
                ;;
        esac
    done

    case "$method" in
        source)
            install_from_source
            ;;
        binary)
            download_release "$os" "$arch" "${version:-latest}"
            ;;
        auto)
            # Try binary first, fall back to source
            download_release "$os" "$arch" "${version:-latest}"
            ;;
    esac

    echo ""
    info "VectorCode installed successfully!"
    info "Run 'vectorcode init' in your project to get started."
    echo ""
}

main "$@"
