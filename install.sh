#!/bin/sh
# Install the latest BeamerX release for this OS and CPU.
set -eu

REPOSITORY=${BEAMERX_REPOSITORY:-moaziat/BeamerX}
VERSION=${BEAMERX_VERSION:-latest}
INSTALL_DIR=${BEAMERX_INSTALL_DIR:-${HOME:-}/.local/bin}

fail() {
    printf 'beamerx installer: %s\n' "$*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

[ -n "${HOME:-}" ] || [ -n "${BEAMERX_INSTALL_DIR:-}" ] || fail 'HOME is not set; set BEAMERX_INSTALL_DIR explicitly'
need curl
need tar
need awk
need mktemp
need install

case $(uname -s) in
    Linux) os=unknown-linux-gnu ;;
    Darwin) os=apple-darwin ;;
    *) fail "unsupported operating system: $(uname -s)" ;;
esac

case $(uname -m) in
    x86_64 | amd64) arch=x86_64 ;;
    arm64 | aarch64) arch=aarch64 ;;
    *) fail "unsupported CPU architecture: $(uname -m)" ;;
esac

if [ "$os" = apple-darwin ] && [ "$arch" != aarch64 ]; then
    fail 'macOS releases currently require Apple Silicon (arm64)'
fi

target=${arch}-${os}
asset=beamerx-${target}.tar.gz
case $VERSION in
    latest) base=https://github.com/${REPOSITORY}/releases/latest/download ;;
    v*) base=https://github.com/${REPOSITORY}/releases/download/${VERSION} ;;
    *) fail 'BEAMERX_VERSION must be "latest" or a tag such as v0.1.0' ;;
esac

download() {
    url=$1
    output=$2
    description=$3
    if ! curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
        "$url" --output "$output"
    then
        fail "could not download $description. Check that https://github.com/${REPOSITORY}/releases has a public $VERSION release for $target"
    fi
}

tmp=$(mktemp -d "${TMPDIR:-/tmp}/beamerx-install.XXXXXX")
cleanup() {
    rm -rf "$tmp"
}
trap cleanup EXIT HUP INT TERM

printf 'Downloading BeamerX for %s...\n' "$target"
download "$base/$asset" "$tmp/$asset" "release asset $asset"
download "$base/SHA256SUMS" "$tmp/SHA256SUMS" 'release checksums'

expected=$(awk -v name="$asset" '$2 == name { print $1; exit }' "$tmp/SHA256SUMS")
[ -n "$expected" ] || fail "release checksum is missing for $asset"
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$asset" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$tmp/$asset" | awk '{ print $1 }')
else
    fail 'sha256sum or shasum is required to verify the download'
fi
[ "$actual" = "$expected" ] || fail "checksum verification failed for $asset"

tar -xzf "$tmp/$asset" -C "$tmp"
binary=$tmp/beamerx-${target}/beamerx
[ -f "$binary" ] || fail 'release archive does not contain the BeamerX binary'

mkdir -p "$INSTALL_DIR"
install -m 755 "$binary" "$INSTALL_DIR/beamerx"

printf 'Installed %s\n' "$INSTALL_DIR/beamerx"
"$INSTALL_DIR/beamerx" --version
case :${PATH}: in
    *:"$INSTALL_DIR":*) ;;
    *)
        printf '\nAdd BeamerX to your PATH, then open a new terminal:\n'
        printf '  export PATH="%s:$PATH"\n' "$INSTALL_DIR"
        ;;
esac
printf '\nRun: beamerx\n'
