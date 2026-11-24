#!/bin/sh
# End-to-end installer contract test with a local, checksum-verified fixture.
set -eu

cd "$(dirname "$0")/.."
tmp=$(mktemp -d "${TMPDIR:-/tmp}/beamerx-installer-test.XXXXXX")
cleanup() {
    rm -rf "$tmp"
}
trap cleanup EXIT HUP INT TERM

case $(uname -s) in
    Linux) os=unknown-linux-gnu ;;
    Darwin) os=apple-darwin ;;
    *) printf 'unsupported test OS\n' >&2; exit 1 ;;
esac
case $(uname -m) in
    x86_64 | amd64) arch=x86_64 ;;
    arm64 | aarch64) arch=aarch64 ;;
    *) printf 'unsupported test architecture\n' >&2; exit 1 ;;
esac
target=${arch}-${os}
asset=beamerx-${target}.tar.gz

mkdir -p "$tmp/stage/beamerx-${target}" "$tmp/release" "$tmp/mock-bin"
cat > "$tmp/stage/beamerx-${target}/beamerx" <<'MOCK_BINARY'
#!/bin/sh
printf 'beamerx installer-test\n'
MOCK_BINARY
chmod +x "$tmp/stage/beamerx-${target}/beamerx"
tar -C "$tmp/stage" -czf "$tmp/release/$asset" "beamerx-${target}"
if command -v sha256sum >/dev/null 2>&1; then
    sum=$(sha256sum "$tmp/release/$asset" | awk '{ print $1 }')
else
    sum=$(shasum -a 256 "$tmp/release/$asset" | awk '{ print $1 }')
fi
printf '%s  %s\n' "$sum" "$asset" > "$tmp/release/SHA256SUMS"

cat > "$tmp/mock-bin/curl" <<'MOCK_CURL'
#!/bin/sh
set -eu
[ "${BEAMERX_TEST_CURL_FAIL:-0}" != 1 ] || exit 22
url=
out=
while [ "$#" -gt 0 ]; do
    case $1 in
        --output) out=$2; shift 2 ;;
        http://* | https://*) url=$1; shift ;;
        *) shift ;;
    esac
done
[ -n "$url" ] && [ -n "$out" ]
cp "$BEAMERX_TEST_RELEASE/${url##*/}" "$out"
MOCK_CURL
chmod +x "$tmp/mock-bin/curl"

BEAMERX_TEST_RELEASE="$tmp/release" \
BEAMERX_INSTALL_DIR="$tmp/bin" \
PATH="$tmp/mock-bin:$PATH" \
    sh install.sh >/dev/null

[ -x "$tmp/bin/beamerx" ]
[ "$("$tmp/bin/beamerx" --version)" = 'beamerx installer-test' ]

if BEAMERX_TEST_CURL_FAIL=1 \
    BEAMERX_INSTALL_DIR="$tmp/failed-bin" \
    PATH="$tmp/mock-bin:$PATH" \
    sh install.sh >"$tmp/failed.out" 2>"$tmp/failed.err"
then
    printf 'installer unexpectedly accepted a failed download\n' >&2
    exit 1
fi
grep -q 'could not download release asset' "$tmp/failed.err"
[ ! -e "$tmp/failed-bin/beamerx" ]

printf 'installer test passed for %s\n' "$target"
