#!/bin/bash
#
# Builds ProxyLab.app from the Swift package.
#
# `swift build` produces an executable, not an application: run it straight and
# AppKit gives it no Dock icon and no menu bar. This wraps that executable in a
# bundle with `Resources/Info.plist`, which is what macOS needs to treat it as
# an application you can double-click, keep in the Dock, and run with no Xcode
# and no terminal.
#
# It also builds the transparent proxy provider and embeds it where macOS
# looks for a system extension:
#
#   ProxyLab.app/Contents/Library/SystemExtensions/
#       <bundle id>.TransparentProxy.systemextension
#
#   ./Scripts/make-app-bundle.sh              release build, into .build/
#   ./Scripts/make-app-bundle.sh /Applications   and copied where you say
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
#   PROXYLAB_SIGNING_IDENTITY     an "Apple Development: …" identity of that team
#   PROXYLAB_APP_PROFILE          a macOS development profile for the app's ID
#   PROXYLAB_EXTENSION_PROFILE    one for <app ID>.TransparentProxy
#
# and optionally PROXYLAB_BUNDLE_ID, when com.proxylab.ProxyLab is not an App
# ID the team can register. Both profiles must grant the Network Extension
# capability (App Proxy), and the app's must also grant System Extension. The
# README says how to make them. Then copy the app to /Applications — macOS
# installs system extensions from nowhere else.
#
# Nothing here installs, approves, or turns on the extension, and nothing
# touches this Mac's proxy settings: that is the app's Turn On, pressed by a
# person.

set -euo pipefail

package_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
destination="${1:-$package_directory/.build}"
bundle="$destination/ProxyLab.app"
resources="$package_directory/Resources"

bundle_id="${PROXYLAB_BUNDLE_ID:-$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$resources/Info.plist")}"
extension_id="$bundle_id.TransparentProxy"

team="${PROXYLAB_TEAM_ID:-}"
identity="${PROXYLAB_SIGNING_IDENTITY:-}"
app_profile="${PROXYLAB_APP_PROFILE:-}"
extension_profile="${PROXYLAB_EXTENSION_PROFILE:-}"

signed=false
if [[ -n "$team$identity$app_profile$extension_profile" ]]; then
    if [[ -z "$team" || -z "$identity" || -z "$app_profile" || -z "$extension_profile" ]]; then
        echo "error: set all four of PROXYLAB_TEAM_ID, PROXYLAB_SIGNING_IDENTITY," >&2
        echo "       PROXYLAB_APP_PROFILE and PROXYLAB_EXTENSION_PROFILE, or none of them" >&2
        exit 1
    fi
    signed=true
fi

# Reads what a provisioning profile grants, and refuses one that is for some
# other App ID or is missing the capability this build needs. Found out here
# rather than as a launch that macOS silently kills.
check_profile() {
    local profile="$1" expected_id="$2" needs_install="$3"
    local decoded granted

    [[ -f "$profile" ]] || { echo "error: no provisioning profile at $profile" >&2; exit 1; }
    decoded="$(security cms -D -i "$profile" 2>/dev/null)" || {
        echo "error: $profile is not a provisioning profile" >&2
        exit 1
    }

    # A macOS profile names its App ID under one key and an iOS profile under
    # another; reading both is what lets the message say which it was.
    granted="$(plutil -extract Entitlements.com\\.apple\\.application-identifier raw -o - - <<<"$decoded" 2>/dev/null ||
        plutil -extract Entitlements.application-identifier raw -o - - <<<"$decoded" 2>/dev/null || true)"
    if [[ "$granted" != "$team.$expected_id" ]]; then
        echo "error: $profile is for '$granted', not '$team.$expected_id'" >&2
        exit 1
    fi
    if ! grep -q "app-proxy-provider" <<<"$decoded"; then
        echo "error: $profile does not grant the Network Extension (App Proxy) capability" >&2
        exit 1
    fi
    if [[ "$needs_install" == yes ]] && ! grep -q "com.apple.developer.system-extension.install" <<<"$decoded"; then
        echo "error: $profile does not grant the System Extension capability" >&2
        exit 1
    fi
}

# A template with the team and bundle identifier written in. An unsigned
# build has no team, and gets a name no team has; nothing is ever installed
# from one.
fill() {
    sed -e "s/__TEAM_ID__/${team:-UNSIGNED}/g" -e "s/__APP_BUNDLE_ID__/$bundle_id/g" "$1" > "$2"
}

if $signed; then
    check_profile "$app_profile" "$bundle_id" yes
    check_profile "$extension_profile" "$extension_id" no
fi

echo "Building ProxyLab and its transparent proxy provider (release)..."
swift build --package-path "$package_directory" --configuration release --product ProxyLab
swift build --package-path "$package_directory" --configuration release --product ProxyLabTransparentProxy

binaries="$(swift build --package-path "$package_directory" --configuration release --show-bin-path)"

echo "Assembling ${bundle}..."
rm -rf "$bundle"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp "$binaries/ProxyLab" "$bundle/Contents/MacOS/ProxyLab"
cp "$resources/Info.plist" "$bundle/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $bundle_id" "$bundle/Contents/Info.plist"

sysext="$bundle/Contents/Library/SystemExtensions/$extension_id.systemextension"
mkdir -p "$sysext/Contents/MacOS"
cp "$binaries/ProxyLabTransparentProxy" "$sysext/Contents/MacOS/ProxyLabTransparentProxy"
fill "$resources/TransparentProxy/Info.plist" "$sysext/Contents/Info.plist"
plutil -lint -s "$sysext/Contents/Info.plist"

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

if $signed; then
    cp "$extension_profile" "$sysext/Contents/embedded.provisionprofile"
    cp "$app_profile" "$bundle/Contents/embedded.provisionprofile"
    fill "$resources/TransparentProxy/TransparentProxy.entitlements" "$scratch/extension.entitlements"
    fill "$resources/ProxyLab.entitlements" "$scratch/app.entitlements"

    # Inside out: the extension is sealed into the app's signature, so it is
    # signed first.
    codesign --force --options runtime --timestamp=none \
        --sign "$identity" --entitlements "$scratch/extension.entitlements" "$sysext"
    codesign --force --options runtime --timestamp=none \
        --sign "$identity" --entitlements "$scratch/app.entitlements" "$bundle"
    codesign --verify --deep --strict "$bundle"

    echo "Built $bundle, signed by $identity for transparent capture"
    echo "Copy it to /Applications and open it from there to install the extension."
else
    # Ad-hoc, so the bundle launches on the machine that built it. No
    # entitlements: a restricted one without a profile is an app macOS will
    # not launch at all.
    codesign --force --sign - "$sysext" >/dev/null 2>&1 &&
        codesign --force --sign - "$bundle" >/dev/null 2>&1 || {
        echo "warning: could not sign the bundle; it may not launch" >&2
    }

    echo "Built $bundle (ad hoc: transparent capture needs a signed build, see the README)"
fi

echo "Start the core first, then: open '$bundle'"
