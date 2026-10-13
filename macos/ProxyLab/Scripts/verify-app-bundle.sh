#!/bin/bash
#
# Checks a ProxyLab.app, or a decoded provisioning profile.
#
#   ./Scripts/verify-app-bundle.sh /path/to/ProxyLab.app
#
# An ad-hoc bundle must contain the extension, carry no restricted
# entitlements, and have no provisioning profile. A development-signed
# bundle must verify under codesign, and the app and the extension must
# carry the entitlements the transparent proxy is built with.
#
#   ./Scripts/verify-app-bundle.sh --decoded-profile profile.plist \
#       --bundle-id com.example.ProxyLab --team AB12CD34EF --kind app
#
# `--decoded-profile` reads a property list that `security cms -D` already
# produced. The packaging test uses it so a check does not need a real
# profile. `make-app-bundle.sh` uses it for the profiles it is about to sign
# with, and again for the profiles embedded in the finished bundle.

set -euo pipefail

die() {
    echo "error: $*" >&2
    exit 1
}

usage() {
    die "usage: verify-app-bundle.sh ProxyLab.app
       verify-app-bundle.sh --decoded-profile FILE --bundle-id ID --team TEAM --kind app|extension"
}

plist_raw() {
    local keypath="$1" file="$2"
    plutil -extract "$keypath" raw -o - -- "$file" 2>/dev/null
}

# True when the JSON array at `keypath` contains `needle` as one element,
# not as a prefix of a longer element.
array_contains() {
    local file="$1" keypath="$2" needle="$3" json
    json="$(plutil -extract "$keypath" json -o - -- "$file" 2>/dev/null)" || return 1
    grep -F -q "\"${needle}\"" <<<"$json"
}

require_string() {
    local file="$1" keypath="$2" expected="$3" what="$4" found
    found="$(plist_raw "$keypath" "$file" || true)"
    [[ "$found" == "$expected" ]] || die "$what: $keypath is '$found', not '$expected'"
}

require_true() {
    local file="$1" keypath="$2" what="$3" found
    found="$(plist_raw "$keypath" "$file" || true)"
    [[ "$found" == "true" ]] || die "$what does not grant ${keypath//\\/}"
}

require_member() {
    local file="$1" keypath="$2" needle="$3" what="$4"
    array_contains "$file" "$keypath" "$needle" || die "$what does not grant $needle"
}

# `profile` is a decoded provisioning profile. `kind` is `app` or `extension`.
check_decoded_profile() {
    local profile="$1" bundle_id="$2" team="$3" kind="$4"
    local granted app_id expiration platform now_epoch expiration_epoch

    plutil -lint -s "$profile" || die "$profile is not a property list"

    granted="$(plist_raw 'Entitlements.com\.apple\.application-identifier' "$profile" || true)"
    if [[ "$granted" != "$team.$bundle_id" ]]; then
        echo "error: $profile is for '$granted', not '$team.$bundle_id'" >&2
        if [[ "$granted" == *'*'* ]]; then
            echo "       A wildcard App ID cannot sign ProxyLab. Make a macOS development" >&2
            echo "       profile whose App ID is exactly $bundle_id." >&2
        fi
        exit 1
    fi

    array_contains "$profile" "TeamIdentifier" "$team" ||
        die "$profile is not from team $team (PROXYLAB_TEAM_ID)"

    platform="$(plutil -extract Platform json -o - -- "$profile" 2>/dev/null || true)"
    if [[ -n "$platform" && "$platform" != *OSX* && "$platform" != *macOS* ]]; then
        die "$profile is not a macOS provisioning profile (platform $platform)"
    fi

    expiration="$(plist_raw "ExpirationDate" "$profile" || true)"
    [[ -n "$expiration" ]] || die "$profile has no expiration date"
    expiration_epoch="$(date -j -f "%Y-%m-%dT%H:%M:%SZ" "$expiration" "+%s" 2>/dev/null || true)"
    [[ -n "$expiration_epoch" ]] || die "$profile has an expiration date this script cannot read ('$expiration')"
    now_epoch="$(date "+%s")"
    [[ "$expiration_epoch" -gt "$now_epoch" ]] || die "$profile has expired ($expiration)"

    require_member "$profile" 'Entitlements.com\.apple\.developer\.networking\.networkextension' \
        "app-proxy-provider" "$profile"
    # The distribution-only value contains this text as a prefix. Matching the
    # quoted element above is what keeps a Developer ID profile from passing.

    if [[ "$kind" == "app" ]]; then
        require_true "$profile" 'Entitlements.com\.apple\.developer\.system-extension\.install' \
            "$profile"
    elif [[ "$kind" == "extension" ]]; then
        [[ "$bundle_id" == *.TransparentProxy ]] ||
            die "extension profile bundle id must end in .TransparentProxy"
        app_id="${bundle_id%.TransparentProxy}"
        # App Sandbox and network.client are unrestricted macOS entitlements
        # (TN3125). A Mac Development profile does not authorize them. The
        # signed extension still receives both from TransparentProxy.entitlements,
        # and check_development_signature requires them on that signature.
        #
        # The explicit App Group is group.<app id>. A team-prefix wildcard
        # such as TEAMID.* is a different entry and does not grant it.
        require_member "$profile" 'Entitlements.com\.apple\.security\.application-groups' \
            "group.$app_id" "$profile"
    else
        die "kind must be app or extension, not '$kind'"
    fi
}

extension_bundle() {
    local app="$1" app_id="$2"
    printf '%s\n' "$app/Contents/Library/SystemExtensions/$app_id.TransparentProxy.systemextension"
}

check_layout() {
    local app="$1" app_id extension_id extension_path count mach
    [[ -d "$app" ]] || die "no application at $app"
    [[ "$(basename "$app")" == "ProxyLab.app" ]] || die "$app is not ProxyLab.app"

    app_id="$(plist_raw "CFBundleIdentifier" "$app/Contents/Info.plist" || true)"
    [[ -n "$app_id" ]] || die "$app has no CFBundleIdentifier"
    [[ "$app_id" != *__* ]] || die "$app still contains a packaging placeholder"
    require_string "$app/Contents/Info.plist" "CFBundleExecutable" "ProxyLab" "$app"
    require_string "$app/Contents/Info.plist" "CFBundlePackageType" "APPL" "$app"
    [[ -x "$app/Contents/MacOS/ProxyLab" ]] || die "$app is missing Contents/MacOS/ProxyLab"
    file "$app/Contents/MacOS/ProxyLab" | grep -q "Mach-O" ||
        die "$app executable is not a Mach-O binary"
    [[ -x "$app/Contents/MacOS/proxy-engine" ]] || die "$app is missing Contents/MacOS/proxy-engine"
    file "$app/Contents/MacOS/proxy-engine" | grep -q "Mach-O" ||
        die "proxy-engine is not a Mach-O executable"

    extension_id="$app_id.TransparentProxy"
    extension_path="$(extension_bundle "$app" "$app_id")"
    [[ -d "$extension_path" ]] ||
        die "no Network Extension bundle at ${extension_path#"$app"/}"

    count="$(find "$app/Contents/Library/SystemExtensions" -name '*.systemextension' -maxdepth 1 | wc -l | tr -d ' ')"
    [[ "$count" == "1" ]] ||
        die "expected one system extension, found $count (a stale extension was left in the bundle)"

    plutil -lint -s "$app/Contents/Info.plist"
    plutil -lint -s "$extension_path/Contents/Info.plist"
    if grep -E -q '__TEAM_ID__|__APP_BUNDLE_ID__' \
        "$app/Contents/Info.plist" "$extension_path/Contents/Info.plist"; then
        die "a bundle property list still contains a packaging placeholder"
    fi

    require_string "$extension_path/Contents/Info.plist" "CFBundleIdentifier" "$extension_id" \
        "the Network Extension"
    require_string "$extension_path/Contents/Info.plist" "CFBundleExecutable" \
        "ProxyLabTransparentProxy" "the Network Extension"
    require_string "$extension_path/Contents/Info.plist" "CFBundlePackageType" "SYSX" \
        "the Network Extension"
    require_string "$extension_path/Contents/Info.plist" \
        'NetworkExtension.NEProviderClasses.com\.apple\.networkextension\.app-proxy' \
        "ProxyLabTransparentProxyProvider" "the Network Extension"
    [[ -x "$extension_path/Contents/MacOS/ProxyLabTransparentProxy" ]] ||
        die "the Network Extension is missing its executable"

    mach="$(plist_raw 'NetworkExtension.NEMachServiceName' "$extension_path/Contents/Info.plist" || true)"
    [[ "$mach" == "group.$app_id.TransparentProxy" ]] ||
        die "NEMachServiceName is '$mach', not 'group.$app_id.TransparentProxy'"

    printf '%s\n' "$extension_path"
}

# `codesign -dv` is verbose level 1. Current macOS prints Authority lines only
# at level 2 or higher, so level 1 makes a development signature look unsigned.
# Ad-hoc signatures still report Signature=adhoc at every level.
signature_text() {
    codesign -d --verbose=2 "$1" 2>&1 || true
}

entitlements_plist() {
    local target="$1" dest="$2" raw
    # Without --xml, current codesign writes an abstract [Dict] dump. That
    # dump has no plist markup, so treating it as the signed entitlements
    # makes a correctly signed bundle look like it has none.
    raw="$(codesign -d --entitlements - --xml "$target" 2>/dev/null || true)"
    # macOS sed treats `\?` as an optional quantifier, so `<\?xml` does not
    # match `<?xml`. Extended regex with an escaped question mark does.
    printf '%s\n' "$raw" | sed -n -E '/<\?xml|<plist/,$p' >"$dest"
    [[ -s "$dest" ]] || die "$target has no signed entitlements"
    plutil -lint -s "$dest"
}

check_adhoc() {
    local app="$1" extension_path="$2" text
    text="$(signature_text "$app")"
    grep -q 'Signature=adhoc' <<<"$text" || die "$app is not ad-hoc signed and not development signed"
    text="$(signature_text "$extension_path")"
    grep -q 'Signature=adhoc' <<<"$text" || die "the Network Extension is not ad-hoc signed"
    text="$(signature_text "$app/Contents/MacOS/proxy-engine")"
    grep -q 'Signature=adhoc' <<<"$text" || die "proxy-engine is not ad-hoc signed"
    codesign --verify --strict "$app/Contents/MacOS/proxy-engine"

    [[ ! -f "$app/Contents/embedded.provisionprofile" ]] ||
        die "an ad-hoc build must not embed a provisioning profile"
    [[ ! -f "$extension_path/Contents/embedded.provisionprofile" ]] ||
        die "an ad-hoc extension must not embed a provisioning profile"

    if codesign -d --entitlements - "$app" 2>/dev/null | grep -E -q 'system-extension.install|app-sandbox'; then
        die "an ad-hoc build carries a restricted entitlement; sign with a profile or sign with none"
    fi

    codesign --verify --deep --strict "$app"
    codesign --verify --strict "$extension_path"
}

check_development_signature() {
    local app="$1" extension_path="$2" scratch="$3"
    local app_id extension_id team app_ents ext_ents app_group groups

    codesign --verify --deep --strict "$app"
    codesign --verify --strict "$extension_path"
    codesign --verify --strict "$app/Contents/MacOS/proxy-engine"

    app_id="$(plist_raw "CFBundleIdentifier" "$app/Contents/Info.plist")"
    extension_id="$app_id.TransparentProxy"
    entitlements_plist "$app" "$scratch/app-entitlements.plist"
    entitlements_plist "$extension_path" "$scratch/extension-entitlements.plist"
    app_ents="$scratch/app-entitlements.plist"
    ext_ents="$scratch/extension-entitlements.plist"

    team="$(plist_raw 'com\.apple\.developer\.team-identifier' "$app_ents" || true)"
    [[ "$team" =~ ^[A-Z0-9]{10}$ ]] || die "signed app has no usable team identifier ('$team')"

    require_string "$app_ents" 'com\.apple\.application-identifier' "$team.$app_id" "the app"
    require_true "$app_ents" 'com\.apple\.developer\.system-extension\.install' "the app"
    require_member "$app_ents" 'com\.apple\.developer\.networking\.networkextension' \
        "app-proxy-provider" "the app"
    if [[ "$(plist_raw 'com\.apple\.security\.app-sandbox' "$app_ents" || true)" == "true" ]]; then
        die "the app is sandboxed; App Sandbox belongs on the Network Extension only"
    fi

    require_string "$ext_ents" 'com\.apple\.developer\.team-identifier' "$team" "the Network Extension"
    require_string "$ext_ents" 'com\.apple\.application-identifier' "$team.$extension_id" \
        "the Network Extension"
    require_member "$ext_ents" 'com\.apple\.developer\.networking\.networkextension' \
        "app-proxy-provider" "the Network Extension"
    require_true "$ext_ents" 'com\.apple\.security\.app-sandbox' "the Network Extension"
    require_true "$ext_ents" 'com\.apple\.security\.network\.client' "the Network Extension"
    app_group="group.$app_id"
    groups="$(plutil -extract 'com\.apple\.security\.application-groups' json -o - -- "$ext_ents" 2>/dev/null || true)"
    [[ "$groups" == "[\"$app_group\"]" ]] ||
        die "the Network Extension application group is '$groups', not '[\"$app_group\"]'"
    require_string "$extension_path/Contents/Info.plist" 'NetworkExtension.NEMachServiceName' \
        "$app_group.TransparentProxy" "the Network Extension"

    [[ -f "$app/Contents/embedded.provisionprofile" ]] ||
        die "the signed app has no embedded provisioning profile"
    [[ -f "$extension_path/Contents/embedded.provisionprofile" ]] ||
        die "the signed Network Extension has no embedded provisioning profile"

    security cms -D -i "$app/Contents/embedded.provisionprofile" >"$scratch/app-profile.plist" 2>/dev/null ||
        die "the app's embedded profile is not a provisioning profile"
    security cms -D -i "$extension_path/Contents/embedded.provisionprofile" >"$scratch/extension-profile.plist" 2>/dev/null ||
        die "the extension's embedded profile is not a provisioning profile"
    check_decoded_profile "$scratch/app-profile.plist" "$app_id" "$team" app
    check_decoded_profile "$scratch/extension-profile.plist" "$extension_id" "$team" extension

    local identifier
    identifier="$(signature_text "$app" | sed -n 's/^Identifier=//p' | head -n 1)"
    [[ "$identifier" == "$app_id" ]] || die "app signature identifier is '$identifier', not '$app_id'"
    identifier="$(signature_text "$extension_path" | sed -n 's/^Identifier=//p' | head -n 1)"
    [[ "$identifier" == "$extension_id" ]] ||
        die "extension signature identifier is '$identifier', not '$extension_id'"
}

verify_app() {
    local app="$1" extension_path scratch text
    extension_path="$(check_layout "$app")"
    text="$(signature_text "$app")"
    if grep -q 'Signature=adhoc' <<<"$text"; then
        check_adhoc "$app" "$extension_path"
        echo "verified $app (ad hoc: no Network Extension entitlements)"
        return
    fi
    if ! grep -q 'Authority=' <<<"$text"; then
        die "$app has no code signature. Re-run Scripts/make-app-bundle.sh."
    fi
    scratch="$(mktemp -d)"
    # The trap runs after this function returns, when the local `scratch` is
    # gone. Capture the path in the trap command so `set -u` does not see an
    # unset variable on the way out of a successful verification.
    trap "rm -rf $(printf '%q' "$scratch")" EXIT
    check_development_signature "$app" "$extension_path" "$scratch"
    rm -rf "$scratch"
    echo "verified $app (development signature, extension entitlements present)"
}

parsed_profile=""
parsed_bundle_id=""
parsed_team=""
parsed_kind=""
if [[ "${1:-}" == "--decoded-profile" ]]; then
    [[ $# -ge 2 ]] || usage
    parsed_profile="$2"
    shift 2
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --bundle-id)
                parsed_bundle_id="${2:-}"
                shift 2
                ;;
            --team)
                parsed_team="${2:-}"
                shift 2
                ;;
            --kind)
                parsed_kind="${2:-}"
                shift 2
                ;;
            *)
                die "unknown argument '$1'"
                ;;
        esac
    done
    [[ -n "$parsed_profile" && -n "$parsed_bundle_id" && -n "$parsed_team" && -n "$parsed_kind" ]] ||
        usage
    [[ -f "$parsed_profile" ]] || die "no decoded profile at $parsed_profile"
    check_decoded_profile "$parsed_profile" "$parsed_bundle_id" "$parsed_team" "$parsed_kind"
    exit 0
fi

[[ $# -eq 1 ]] || usage
verify_app "$1"
