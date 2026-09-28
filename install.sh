#!/usr/bin/env sh
# Install bddkit from a GitHub Release. Linux/macOS only — see install.ps1 for Windows.
#
#   curl -fsSL https://raw.githubusercontent.com/bddkit/bddkit/main/install.sh | sh
#
# Adapted from starship's install.sh (MIT): https://github.com/starship/starship

set -eu
printf '\n'

BOLD="$(tput bold 2>/dev/null || printf '')"
GREY="$(tput setaf 0 2>/dev/null || printf '')"
RED="$(tput setaf 1 2>/dev/null || printf '')"
GREEN="$(tput setaf 2 2>/dev/null || printf '')"
YELLOW="$(tput setaf 3 2>/dev/null || printf '')"
UNDERLINE="$(tput smul 2>/dev/null || printf '')"
NO_COLOR="$(tput sgr0 2>/dev/null || printf '')"

REPO="bddkit/bddkit"
SUPPORTED_TARGETS="x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-apple-darwin aarch64-apple-darwin"

info() { printf '%s\n' "${BOLD}${GREY}>${NO_COLOR} $*"; }
warn() { printf '%s\n' "${YELLOW}! $*${NO_COLOR}"; }
error() { printf '%s\n' "${RED}x $*${NO_COLOR}" >&2; }
completed() { printf '%s\n' "${GREEN}✓${NO_COLOR} $*"; }
has() { command -v "$1" 1>/dev/null 2>&1; }

# Maps `uname -s`/`uname -m` onto one of the four Rust target triples the
# release workflow actually builds (.github/workflows/release.yml). Anything
# else is refused by name rather than guessed at.
detect_target() {
	os="$(uname -s)"
	arch="$(uname -m)"

	case "$arch" in
	x86_64 | amd64) arch="x86_64" ;;
	arm64 | aarch64) arch="aarch64" ;;
	esac

	case "$os" in
	Linux) printf '%s-unknown-linux-gnu' "$arch" ;;
	Darwin) printf '%s-apple-darwin' "$arch" ;;
	*) printf '%s-%s' "$arch" "$os" ;;
	esac
}

is_supported() {
	target="$1"
	for t in $SUPPORTED_TARGETS; do
		[ "$t" = "$target" ] && return 0
	done
	return 1
}

download() {
	dest="$1"
	url="$2"
	if has curl; then
		curl --fail --silent --location --output "$dest" "$url"
	elif has wget; then
		wget --quiet --output-document="$dest" "$url"
	else
		error "Neither curl nor wget found — install one and try again."
		exit 1
	fi
}

# GitHub's /releases/latest page 302s to /releases/tag/<tag>; the tag name is
# the last path segment. No GitHub API call, no rate limit, no token.
resolve_latest_tag() {
	url="https://github.com/${REPO}/releases/latest"
	location=""
	if has curl; then
		location="$(curl -fsSL -o /dev/null -w '%{url_effective}' "$url")"
	elif has wget; then
		location="$(wget --quiet --max-redirect=10 --spider --server-response "$url" 2>&1 \
			| awk '/^  Location: /{loc=$2} END{print loc}')"
	else
		error "Neither curl nor wget found — install one and try again."
		exit 1
	fi
	tag="${location##*/}"
	if [ -z "$tag" ] || [ "$tag" = "latest" ]; then
		error "Could not resolve the latest release tag from ${url}"
		exit 1
	fi
	printf '%s' "$tag"
}

BIN_DIR="${BDDKIT_INSTALL_DIR:-$HOME/.local/bin}"

TARGET="$(detect_target)"
if ! is_supported "$TARGET"; then
	error "No prebuilt bddkit binary for ${TARGET}."
	info "Supported targets: ${SUPPORTED_TARGETS}"
	info "Build from source instead: cargo install --git https://github.com/${REPO}"
	exit 1
fi

info "Resolving latest release…"
TAG="$(resolve_latest_tag)"

NAME="bddkit-${TAG}-${TARGET}"
BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"
ARCHIVE="$(mktemp).tar.gz"

info "Installing bddkit ${TAG} (${TARGET}) to ${BIN_DIR}"

download "$ARCHIVE" "${BASE_URL}/${NAME}.tar.gz"
download "${ARCHIVE}.sha256" "${BASE_URL}/${NAME}.tar.gz.sha256"

if has sha256sum; then
	expected="$(awk '{print $1}' "${ARCHIVE}.sha256")"
	actual="$(sha256sum "$ARCHIVE" | awk '{print $1}')"
elif has shasum; then
	expected="$(awk '{print $1}' "${ARCHIVE}.sha256")"
	actual="$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')"
else
	warn "No sha256sum/shasum found — skipping checksum verification."
	expected="skip"
	actual="skip"
fi

if [ "$expected" != "$actual" ]; then
	error "Checksum mismatch for ${NAME}.tar.gz (expected ${expected}, got ${actual})"
	rm -f "$ARCHIVE" "${ARCHIVE}.sha256"
	exit 1
fi

mkdir -p "$BIN_DIR"
UNPACK_DIR="$(mktemp -d)"
tar -xzf "$ARCHIVE" -C "$UNPACK_DIR"
cp "${UNPACK_DIR}/${NAME}/bddkit" "${BIN_DIR}/bddkit"
chmod +x "${BIN_DIR}/bddkit"
rm -rf "$UNPACK_DIR" "$ARCHIVE" "${ARCHIVE}.sha256"

completed "bddkit ${TAG} installed to ${BIN_DIR}/bddkit"

case ":$PATH:" in
*":${BIN_DIR}:"*) ;;
*)
	printf '\n'
	warn "${BIN_DIR} is not in your \$PATH"
	info "Add this to your shell profile:"
	info "  ${UNDERLINE}export PATH=\"${BIN_DIR}:\$PATH\"${NO_COLOR}"
	;;
esac

printf '\n'
