#!/bin/bash
#
# Builds ProxyLab.app from the Swift package.
#
# `swift build` produces an executable, not an application: run it straight and
# AppKit gives it no Dock icon and no menu bar. This wraps that executable in
# a bundle with `Resources/Info.plist`, which is what macOS needs to treat it
# as an application you can double-click, keep in the Dock, and run with no
# Xcode and no terminal.
#
# It also builds the transparent proxy provider and embeds it where macOS
# looks for a system extension:
#
#   ProxyLab.app/Contents/Library/SystemExtensions/
#       <bundle id>.TransparentProxy.systemextension
#
# And it builds the release proxy-engine and embeds that executable where
# the app launches it from:
#
#   ProxyLab.app/Contents/MacOS/proxy-engine
#
# The app starts that binary itself. Nothing in this script runs it.
#
#   ./Scripts/make-app-bundle.sh              release build, into .build/
#   ./Scripts/make-app-bundle.sh /Applications   and copied where you say
#
# The argument is a directory. The app is always <directory>/ProxyLab.app.
# Pass /Applications to put it where macOS will install a system extension.
# Nothing here installs, approves, or turns on the extension, and nothing
# touches this Mac's proxy settings.
#
# Two ways to sign it.
#
# Ad hoc, with nothing set. What a locally built developer tool needs to
# launch, and exactly what this script always did. The provider is in the
# bundle but cannot be installed: installing a Network Extension takes
# restricted entitlements, which macOS only honours with a provisioning
# profile. The window says so, and everything else works.
#
# For transparent capture, with all four of these set:
#
#   PROXYLAB_TEAM_ID              the Apple Developer team, e.g. AB12CD34EF
#   PROXYLAB_SIGNING_IDENTITY     the codesigning identity, as
#                                 `security find-identity -v -p codesigning` prints it
#   PROXYLAB_APP_PROFILE          a macOS development profile for the app's ID
#   PROXYLAB_EXTENSION_PROFILE    one for <app ID>.TransparentProxy
#
# The team is the certificate's organizational unit. It is not read from the
# identity's display name: a Mac Developer certificate's parentheses often
# hold a different id.
#
# PROXYLAB_BUNDLE_ID is optional, when com.proxylab.ProxyLab is not an App
# ID the team can register. Both profiles must grant the Network Extension
# capability (App Proxy). The app's must also grant System Extension. The
# extension's must grant the app group group.<app id>. The Mach service is
# that group followed by .TransparentProxy. App Sandbox and network client
# are unrestricted on macOS (TN3125); the extension is signed with them from
# its entitlements file. The README says how to make the profiles.
#
# The bundle is assembled in a temporary directory and replaces any app
# already at the destination only after it has been signed and checked. A
# failure leaves the previous app where it was.

set -euo pipefail

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
package_directory="$(cd "$script_directory/.." && pwd)"
check_identity_only=false
if [[ "${1:-}" == "--check-identity" ]]; then
    check_identity_only=true
    shift
fi
destination="${1:-$package_directory/.build}"
resources="$package_directory/Resources"
verify="$script_directory/verify-app-bundle.sh"

die() {
    echo "error: $*" >&2
    exit 1
}

if $check_identity_only && [[ $# -ne 0 ]]; then
    die "--check-identity does not take a destination"
fi

if [[ "$destination" == *.app ]]; then
    die "pass the directory to put ProxyLab.app in, not the .app itself"
fi

bundle_id="${PROXYLAB_BUNDLE_ID:-$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$resources/Info.plist")}"
extension_id="$bundle_id.TransparentProxy"

team="${PROXYLAB_TEAM_ID:-}"
identity="${PROXYLAB_SIGNING_IDENTITY:-}"
app_profile="${PROXYLAB_APP_PROFILE:-}"
extension_profile="${PROXYLAB_EXTENSION_PROFILE:-}"

if [[ ! "$bundle_id" =~ ^[A-Za-z0-9][A-Za-z0-9.-]+$ ]]; then
    die "PROXYLAB_BUNDLE_ID must be a bundle identifier (letters, digits, dots, hyphens)"
fi

signed=false
if ! $check_identity_only && [[ -n "$team$identity$app_profile$extension_profile" ]]; then
    if [[ -z "$team" || -z "$identity" || -z "$app_profile" || -z "$extension_profile" ]]; then
        echo "error: set all four of PROXYLAB_TEAM_ID, PROXYLAB_SIGNING_IDENTITY," >&2
        echo "       PROXYLAB_APP_PROFILE and PROXYLAB_EXTENSION_PROFILE, or none of them" >&2
        exit 1
    fi
    signed=true
fi

# A template with the team and bundle identifier written in. An unsigned
# build has no team, and gets a name no team has; nothing is ever installed
# from one.
fill() {
    sed -e "s/__TEAM_ID__/${team:-UNSIGNED}/g" -e "s/__APP_BUNDLE_ID__/$bundle_id/g" "$1" >"$2"
}

decode_profile() {
    local profile="$1" dest="$2"
    [[ -f "$profile" ]] || die "no provisioning profile at $profile"
    security cms -D -i "$profile" >"$dest" 2>/dev/null ||
        die "$profile is not a provisioning profile"
    plutil -lint -s "$dest"
}

# SHA-1 printed by `security find-identity` for the identity whose quoted name
# is exactly `name`. Empty when that identity is not a codesigning identity.
identity_sha1() {
    local listing="$1" name="$2"
    awk -v name="$name" '
        {
            q = index($0, "\"")
            if (q == 0) next
            rest = substr($0, q + 1)
            end = index(rest, "\"")
            if (end == 0) next
            id = substr(rest, 1, end - 1)
            if (id != name) next
            if ($2 ~ /^[0-9A-Fa-f]{40}$/) {
                print toupper($2)
                exit
            }
        }
    ' <<<"$listing"
}

# Organizational units of a certificate, one per line. Apple puts the team id
# in OU. The display name is not consulted.
certificate_organizational_units() {
    local file="$1" subject
    subject="$(openssl x509 -in "$file" -noout -subject -nameopt multiline,sname 2>/dev/null)" ||
        die "$file is not a certificate"
    printf '%s\n' "$subject" | sed -n 's/^[[:space:]]*OU[[:space:]]*=[[:space:]]*//p'
}

require_certificate_team() {
    local file="$1" units unit found="" shown
    units="$(certificate_organizational_units "$file")"
    while IFS= read -r unit; do
        if [[ "$unit" == "$team" ]]; then
            found=yes
            break
        fi
    done <<<"$units"
    if [[ "$found" == yes ]]; then
        return 0
    fi
    shown="$(printf '%s' "$units" | tr '\n' ',' | sed 's/,$//')"
    echo "error: PROXYLAB_SIGNING_IDENTITY does not belong to team $team." >&2
    echo "       The identity is '$identity'." >&2
    echo "       Its certificate organizational unit is '${shown:-<none>}', not '$team'." >&2
    exit 1
}

# Writes the PEM whose SHA-1 is the codesigning identity `hash`.
certificate_for_identity() {
    local name="$1" hash="$2" dest="$3" pem line block fingerprint
    pem="$(security find-certificate -a -p -c "$name" 2>/dev/null || true)"
    [[ -n "$pem" ]] || return 1
    block=""
    while IFS= read -r line || [[ -n "$line" ]]; do
        block+="$line"$'\n'
        if [[ "$line" == "-----END CERTIFICATE-----" ]]; then
            fingerprint="$(printf '%s' "$block" | openssl x509 -fingerprint -sha1 -noout 2>/dev/null || true)"
            fingerprint="${fingerprint#*=}"
            fingerprint="$(printf '%s' "$fingerprint" | tr -d ':' | tr '[:lower:]' '[:upper:]')"
            if [[ -n "$fingerprint" && "$fingerprint" == "$hash" ]]; then
                printf '%s' "$block" >"$dest"
                return 0
            fi
            block=""
        fi
    done <<<"$pem"
    return 1
}

check_identity() {
    local found hash
    if [[ -z "$identity" ]]; then
        die "PROXYLAB_SIGNING_IDENTITY is required"
    fi
    if [[ ! "$team" =~ ^[A-Z0-9]{10}$ ]]; then
        die "PROXYLAB_TEAM_ID must be the 10-character Apple team id, not a name or a path"
    fi

    # --check-identity may be given a PEM so the packaging test does not need
    # a login-keychain identity. A normal build ignores that variable and
    # reads the certificate of the codesigning identity.
    if $check_identity_only && [[ -n "${PROXYLAB_SIGNING_CERTIFICATE:-}" ]]; then
        [[ -f "$PROXYLAB_SIGNING_CERTIFICATE" ]] ||
            die "no certificate at $PROXYLAB_SIGNING_CERTIFICATE"
        require_certificate_team "$PROXYLAB_SIGNING_CERTIFICATE"
        return 0
    fi

    found="$(security find-identity -v -p codesigning 2>/dev/null || true)"
    hash="$(identity_sha1 "$found" "$identity")"
    if [[ -z "$hash" ]]; then
        echo "error: PROXYLAB_SIGNING_IDENTITY is not in this login keychain." >&2
        echo "       Looked for: $identity" >&2
        echo "       Codesigning identities on this Mac:" >&2
        if [[ -z "$found" ]]; then
            echo "         (none)" >&2
        else
            sed -n 's/.*"\(.*\)".*/         \1/p' <<<"$found" >&2
        fi
        exit 1
    fi

    identity_cert="$(mktemp)"
    if ! certificate_for_identity "$identity" "$hash" "$identity_cert"; then
        die "could not read the certificate for '$identity'"
    fi
    require_certificate_team "$identity_cert"
    rm -f "$identity_cert"
    identity_cert=""
}

stage_root=""
profile_scratch=""
entitlements_dir=""
identity_cert=""
cleanup() {
    [[ -n "$stage_root" && -d "$stage_root" ]] && rm -rf "$stage_root"
    [[ -n "$profile_scratch" && -d "$profile_scratch" ]] && rm -rf "$profile_scratch"
    [[ -n "$entitlements_dir" && -d "$entitlements_dir" ]] && rm -rf "$entitlements_dir"
    [[ -n "$identity_cert" && -f "$identity_cert" ]] && rm -f "$identity_cert"
    return 0
}
trap cleanup EXIT

if $check_identity_only; then
    [[ -n "$team" && -n "$identity" ]] ||
        die "PROXYLAB_TEAM_ID and PROXYLAB_SIGNING_IDENTITY are required"
    check_identity
    exit 0
fi

if $signed; then
    profile_scratch="$(mktemp -d)"
    decode_profile "$app_profile" "$profile_scratch/app.plist"
    decode_profile "$extension_profile" "$profile_scratch/extension.plist"
    "$verify" --decoded-profile "$profile_scratch/app.plist" \
        --bundle-id "$bundle_id" --team "$team" --kind app
    "$verify" --decoded-profile "$profile_scratch/extension.plist" \
        --bundle-id "$extension_id" --team "$team" --kind extension
    rm -rf "$profile_scratch"
    profile_scratch=""
    check_identity
fi

repository_root="$(cd "$package_directory/../.." && pwd)"
if ! command -v cargo >/dev/null 2>&1; then
    die "cargo is required to build the proxy-engine embedded in ProxyLab.app"
fi

echo "Building proxy-engine (release)..."
cargo build --release --locked --manifest-path "$repository_root/core/Cargo.toml" --bin proxy-engine
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
    engine="${CARGO_TARGET_DIR}/release/proxy-engine"
else
    engine="$repository_root/core/target/release/proxy-engine"
fi
[[ -x "$engine" ]] || die "proxy-engine was not produced at $engine"

echo "Building ProxyLab and its transparent proxy provider (release)..."
swift build --package-path "$package_directory" --configuration release --product ProxyLab
swift build --package-path "$package_directory" --configuration release --product ProxyLabTransparentProxy

binaries="$(swift build --package-path "$package_directory" --configuration release --show-bin-path)"

stage_root="$(mktemp -d "${TMPDIR:-/tmp}/proxylab-bundle.XXXXXX")"
bundle="$stage_root/ProxyLab.app"

echo "Assembling ${bundle}..."
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp "$binaries/ProxyLab" "$bundle/Contents/MacOS/ProxyLab"
cp "$engine" "$bundle/Contents/MacOS/proxy-engine"
cp "$resources/Info.plist" "$bundle/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $bundle_id" "$bundle/Contents/Info.plist"
chmod +x "$bundle/Contents/MacOS/ProxyLab" "$bundle/Contents/MacOS/proxy-engine"

sysext="$bundle/Contents/Library/SystemExtensions/$extension_id.systemextension"
mkdir -p "$sysext/Contents/MacOS"
cp "$binaries/ProxyLabTransparentProxy" "$sysext/Contents/MacOS/ProxyLabTransparentProxy"
fill "$resources/TransparentProxy/Info.plist" "$sysext/Contents/Info.plist"
chmod +x "$sysext/Contents/MacOS/ProxyLabTransparentProxy"
plutil -lint -s "$sysext/Contents/Info.plist"

# Inside out. Signing the app with --deep would re-sign the extension and
# drop the entitlements just applied to it, so the extension is signed first
# and the app is signed without --deep.
if $signed; then
    entitlements_dir="$(mktemp -d)"
    cp "$extension_profile" "$sysext/Contents/embedded.provisionprofile"
    cp "$app_profile" "$bundle/Contents/embedded.provisionprofile"
    fill "$resources/TransparentProxy/TransparentProxy.entitlements" "$entitlements_dir/extension.plist"
    fill "$resources/ProxyLab.entitlements" "$entitlements_dir/app.plist"

    # Recent codesign wants a DER copy of the entitlements. Passing the flag
    # on a codesign that does not have it would fail the signed build.
    der=""
    if codesign --help 2>&1 | grep -q "generate-entitlement-der"; then
        der="--generate-entitlement-der"
    fi

    # The helper is signed on its own, with no entitlements: it is a normal
    # executable the app launches, not the Network Extension. Signing the app
    # afterwards does not use --deep, which would strip the extension.
    # No entitlements on the helper, so no DER entitlements either.
    codesign --force --options runtime --timestamp=none \
        --sign "$identity" "$bundle/Contents/MacOS/proxy-engine" >/dev/null
    # shellcheck disable=SC2086
    codesign --force --options runtime --timestamp=none $der \
        --sign "$identity" --entitlements "$entitlements_dir/extension.plist" "$sysext" >/dev/null
    # shellcheck disable=SC2086
    codesign --force --options runtime --timestamp=none $der \
        --sign "$identity" --entitlements "$entitlements_dir/app.plist" "$bundle" >/dev/null
    rm -rf "$entitlements_dir"
    entitlements_dir=""
else
    # Ad-hoc, so the bundle launches on the machine that built it. No
    # entitlements: a restricted one without a profile is an app macOS will
    # not launch at all.
    codesign --force --sign - "$bundle/Contents/MacOS/proxy-engine" >/dev/null
    codesign --force --sign - "$sysext" >/dev/null
    codesign --force --sign - "$bundle" >/dev/null
fi

"$verify" "$bundle"

final="$destination/ProxyLab.app"
backup="$destination/ProxyLab.app.replacing"
mkdir -p "$destination"
rm -rf "$backup"
if [[ -d "$final" ]]; then
    mv "$final" "$backup"
fi
if mv "$bundle" "$final"; then
    rm -rf "$backup"
else
    if [[ -d "$backup" ]]; then
        mv "$backup" "$final" || true
    fi
    die "could not replace $final; the previous copy was left in place if there was one"
fi

if $signed; then
    echo "Built $final"
    echo "Signed development build ($identity)."
    echo "Proxy engine: Contents/MacOS/proxy-engine"
    echo "Network Extension: Contents/Library/SystemExtensions/$extension_id.systemextension"
    echo "macOS installs that extension only from /Applications."
    echo "This script did not enable it, start it, or change any proxy setting."
else
    echo "Built $final"
    echo "Ad-hoc build: the app launches and starts the bundled proxy engine."
    echo "Transparent capture stays unavailable until the app is signed."
    echo "The Network Extension is embedded, without a provisioning profile."
    echo "Sign with PROXYLAB_TEAM_ID and the two profiles to install it — see the README."
fi
echo "Open '$final'. The app starts and stops its own proxy engine."
