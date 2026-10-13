#!/bin/bash
#
# Checks the packaging rules without an Apple Developer account and without
# building ProxyLab. A fake ad-hoc bundle exercises the layout check. Decoded
# profile plists exercise the mismatches a signed build must refuse.

set -euo pipefail

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
package_directory="$(cd "$script_directory/.." && pwd)"
verify="$script_directory/verify-app-bundle.sh"
make_bundle="$script_directory/make-app-bundle.sh"
resources="$package_directory/Resources"
root="$(mktemp -d)"
failures=0
signing_keychain=""

cleanup() {
    if [[ -n "$signing_keychain" ]]; then
        security delete-keychain "$signing_keychain" >/dev/null 2>&1 || true
    fi
    rm -rf "$root"
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*" >&2
    failures=$((failures + 1))
}

expect_ok() {
    local description="$1"
    shift
    if ! "$@" >"$root/out" 2>"$root/err"; then
        fail "$description"
        sed 's/^/    /' "$root/err" >&2
    fi
}

expect_message() {
    local description="$1" pattern="$2"
    shift 2
    local err="$root/err"
    if "$@" >"$root/out" 2>"$err"; then
        fail "$description (command succeeded)"
        return
    fi
    if ! grep -q "$pattern" "$err"; then
        fail "$description (message did not match /$pattern/)"
        sed 's/^/    /' "$err" >&2
    fi
}

app_profile() {
    local path="$1" app_id="$2" team="$3" network="$4" install="$5" expires="$6" platform="$7"
    cat >"$path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>TeamIdentifier</key>
    <array><string>$team</string></array>
    <key>ExpirationDate</key>
    <date>$expires</date>
    <key>Platform</key>
    <array><string>$platform</string></array>
    <key>Entitlements</key>
    <dict>
        <key>com.apple.application-identifier</key>
        <string>$team.$app_id</string>
        <key>com.apple.developer.system-extension.install</key>
        <$install/>
        <key>com.apple.developer.networking.networkextension</key>
        <array><string>$network</string></array>
    </dict>
</dict>
</plist>
EOF
}

# A decoded Mac Development extension profile. App Sandbox and network.client
# are unrestricted on macOS and are omitted unless the sixth argument is
# `include-sandbox`. The fifth argument replaces app-proxy-provider when a
# test needs a profile that lacks it. The seventh argument is extra
# `<string>` entries in the application-groups array.
extension_profile() {
    local path="$1" extension_id="$2" team="$3" group="$4"
    local network="${5:-app-proxy-provider}"
    local sandbox_keys=""
    local extra_groups="${7:-}"
    if [[ "${6:-}" == "include-sandbox" ]]; then
        sandbox_keys="$(cat <<'KEYS'
        <key>com.apple.security.app-sandbox</key>
        <true/>
        <key>com.apple.security.network.client</key>
        <true/>
KEYS
)"
    fi
    cat >"$path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>TeamIdentifier</key>
    <array><string>$team</string></array>
    <key>ExpirationDate</key>
    <date>2099-01-01T00:00:00Z</date>
    <key>Platform</key>
    <array><string>OSX</string></array>
    <key>Entitlements</key>
    <dict>
        <key>com.apple.application-identifier</key>
        <string>$team.$extension_id</string>
        <key>com.apple.developer.networking.networkextension</key>
        <array><string>$network</string></array>
$sandbox_keys
        <key>com.apple.security.application-groups</key>
        <array>
            <string>$group</string>
$extra_groups
        </array>
    </dict>
</dict>
</plist>
EOF
}

build_fake_app() {
    local app="$1" binary
    binary="$root/fake-binary"
    if [[ ! -x "$binary" ]]; then
        echo 'int main(void) { return 0; }' | clang -x c -o "$binary" -
    fi

    mkdir -p "$app/Contents/MacOS"
    cp "$resources/Info.plist" "$app/Contents/Info.plist"
    cp "$binary" "$app/Contents/MacOS/ProxyLab"
    cp "$binary" "$app/Contents/MacOS/proxy-engine"
    chmod +x "$app/Contents/MacOS/ProxyLab" "$app/Contents/MacOS/proxy-engine"

    local extension="$app/Contents/Library/SystemExtensions/com.proxylab.ProxyLab.TransparentProxy.systemextension"
    mkdir -p "$extension/Contents/MacOS"
    cp "$binary" "$extension/Contents/MacOS/ProxyLabTransparentProxy"
    chmod +x "$extension/Contents/MacOS/ProxyLabTransparentProxy"
    sed -e 's/__TEAM_ID__/UNSIGNED/g' -e 's/__APP_BUNDLE_ID__/com.proxylab.ProxyLab/g' \
        "$resources/TransparentProxy/Info.plist" >"$extension/Contents/Info.plist"
    codesign --force --sign - "$app/Contents/MacOS/proxy-engine"
    codesign --force --sign - "$extension"
    codesign --force --sign - "$app"
}

echo "Partial signing variables are refused before a build..."
expect_message "one signing variable" "set all four" \
    env PROXYLAB_TEAM_ID=AB12CD34EF \
        PROXYLAB_SIGNING_IDENTITY= \
        PROXYLAB_APP_PROFILE= \
        PROXYLAB_EXTENSION_PROFILE= \
        "$make_bundle" "$root/should-not-build"

if [[ -d "$root/should-not-build/ProxyLab.app" ]]; then
    fail "a partial signing environment still produced an app"
fi

identity_certificate() {
    local path="$1" ou="$2" common_name="$3" config
    config="$root/identity-openssl.cnf"
    cat >"$config" <<EOF
[req]
distinguished_name = dn
x509_extensions = v3
prompt = no
[dn]
CN = $common_name
OU = $ou
O = ProxyLab Test
C = US
[v3]
keyUsage = digitalSignature
extendedKeyUsage = codeSigning
EOF
    openssl req -new -x509 -days 2 -nodes -config "$config" \
        -keyout "$root/identity-key.pem" -out "$path" >/dev/null 2>&1
}

echo "A Mac Developer display name whose parentheses are not the team is accepted..."
identity_certificate "$root/mac-developer.pem" "AB12CD34EF" \
    "Mac Developer: Test Person (4WRGLCZ743)"
expect_ok "display-name suffix differs from team" \
    env PROXYLAB_TEAM_ID=AB12CD34EF \
        PROXYLAB_SIGNING_IDENTITY="Mac Developer: Test Person (4WRGLCZ743)" \
        PROXYLAB_SIGNING_CERTIFICATE="$root/mac-developer.pem" \
        "$make_bundle" --check-identity

echo "An Apple Development identity is accepted from its certificate OU..."
identity_certificate "$root/apple-development.pem" "AB12CD34EF" \
    "Apple Development: Test Person (ZZ99YY88XX)"
expect_ok "apple development parentheses are not the team" \
    env PROXYLAB_TEAM_ID=AB12CD34EF \
        PROXYLAB_SIGNING_IDENTITY="Apple Development: Test Person (ZZ99YY88XX)" \
        PROXYLAB_SIGNING_CERTIFICATE="$root/apple-development.pem" \
        "$make_bundle" --check-identity

echo "A Developer ID Application identity is accepted from its certificate OU..."
identity_certificate "$root/developer-id.pem" "AB12CD34EF" \
    "Developer ID Application: Test Person (ZZ99YY88XX)"
expect_ok "developer id display name is not the team check" \
    env PROXYLAB_TEAM_ID=AB12CD34EF \
        PROXYLAB_SIGNING_IDENTITY="Developer ID Application: Test Person (ZZ99YY88XX)" \
        PROXYLAB_SIGNING_CERTIFICATE="$root/developer-id.pem" \
        "$make_bundle" --check-identity

echo "A certificate whose organizational unit is another team is refused..."
identity_certificate "$root/wrong-ou.pem" "ZZ99YY88XX" \
    "Mac Developer: Test Person (4WRGLCZ743)"
expect_message "wrong certificate OU" "organizational unit is 'ZZ99YY88XX'" \
    env PROXYLAB_TEAM_ID=AB12CD34EF \
        PROXYLAB_SIGNING_IDENTITY="Mac Developer: Test Person (4WRGLCZ743)" \
        PROXYLAB_SIGNING_CERTIFICATE="$root/wrong-ou.pem" \
        "$make_bundle" --check-identity

echo "A development profile for another team is refused..."
app_profile "$root/wrong-team.plist" "com.proxylab.ProxyLab" "ZZ99YY88XX" \
    "app-proxy-provider" true "2099-01-01T00:00:00Z" "OSX"
expect_message "wrong team" "not 'AB12CD34EF.com.proxylab.ProxyLab'" \
    "$verify" --decoded-profile "$root/wrong-team.plist" \
    --bundle-id com.proxylab.ProxyLab --team AB12CD34EF --kind app

echo "A distribution-only network entitlement is not treated as App Proxy..."
app_profile "$root/distribution.plist" "com.proxylab.ProxyLab" "AB12CD34EF" \
    "app-proxy-provider-systemextension" true "2099-01-01T00:00:00Z" "OSX"
expect_message "distribution entitlement" "does not grant app-proxy-provider" \
    "$verify" --decoded-profile "$root/distribution.plist" \
    --bundle-id com.proxylab.ProxyLab --team AB12CD34EF --kind app

echo "An app profile without System Extension is refused..."
app_profile "$root/no-install.plist" "com.proxylab.ProxyLab" "AB12CD34EF" \
    "app-proxy-provider" false "2099-01-01T00:00:00Z" "OSX"
expect_message "missing system extension" "system-extension.install" \
    "$verify" --decoded-profile "$root/no-install.plist" \
    --bundle-id com.proxylab.ProxyLab --team AB12CD34EF --kind app

echo "An expired profile is refused..."
app_profile "$root/expired.plist" "com.proxylab.ProxyLab" "AB12CD34EF" \
    "app-proxy-provider" true "2000-01-01T00:00:00Z" "OSX"
expect_message "expired" "has expired" \
    "$verify" --decoded-profile "$root/expired.plist" \
    --bundle-id com.proxylab.ProxyLab --team AB12CD34EF --kind app

echo "An iOS profile is refused..."
app_profile "$root/ios.plist" "com.proxylab.ProxyLab" "AB12CD34EF" \
    "app-proxy-provider" true "2099-01-01T00:00:00Z" "iOS"
expect_message "ios profile" "not a macOS provisioning profile" \
    "$verify" --decoded-profile "$root/ios.plist" \
    --bundle-id com.proxylab.ProxyLab --team AB12CD34EF --kind app

echo "A matching app profile is accepted..."
app_profile "$root/good-app.plist" "com.proxylab.ProxyLab" "AB12CD34EF" \
    "app-proxy-provider" true "2099-01-01T00:00:00Z" "OSX"
expect_ok "good app profile" \
    "$verify" --decoded-profile "$root/good-app.plist" \
    --bundle-id com.proxylab.ProxyLab --team AB12CD34EF --kind app

echo "An extension profile with the wrong app group is refused..."
extension_profile "$root/bad-group.plist" "com.proxylab.ProxyLab.TransparentProxy" \
    "AB12CD34EF" "AB12CD34EF.other.App"
expect_message "wrong app group" "does not grant group.com.proxylab.ProxyLab" \
    "$verify" --decoded-profile "$root/bad-group.plist" \
    --bundle-id com.proxylab.ProxyLab.TransparentProxy --team AB12CD34EF --kind extension

echo "A team-prefixed app group is not the explicit group..."
extension_profile "$root/team-group.plist" "com.proxylab.ProxyLab.TransparentProxy" \
    "AB12CD34EF" "AB12CD34EF.com.proxylab.ProxyLab"
expect_message "team-prefixed app group" "does not grant group.com.proxylab.ProxyLab" \
    "$verify" --decoded-profile "$root/team-group.plist" \
    --bundle-id com.proxylab.ProxyLab.TransparentProxy --team AB12CD34EF --kind extension

echo "A team-id wildcard does not satisfy the explicit app group..."
extension_profile "$root/wildcard-group.plist" "com.proxylab.ProxyLab.TransparentProxy" \
    "AB12CD34EF" "AB12CD34EF.*"
expect_message "wildcard app group" "does not grant group.com.proxylab.ProxyLab" \
    "$verify" --decoded-profile "$root/wildcard-group.plist" \
    --bundle-id com.proxylab.ProxyLab.TransparentProxy --team AB12CD34EF --kind extension

echo "An extension profile without app-proxy-provider is refused..."
extension_profile "$root/extension-distribution.plist" "com.proxylab.ProxyLab.TransparentProxy" \
    "AB12CD34EF" "group.com.proxylab.ProxyLab" "app-proxy-provider-systemextension"
expect_message "extension distribution entitlement" "does not grant app-proxy-provider" \
    "$verify" --decoded-profile "$root/extension-distribution.plist" \
    --bundle-id com.proxylab.ProxyLab.TransparentProxy --team AB12CD34EF --kind extension

echo "A Mac Development extension profile omits unrestricted sandbox entitlements..."
extension_profile "$root/good-extension.plist" "com.proxylab.ProxyLab.TransparentProxy" \
    "AB12CD34EF" "group.com.proxylab.ProxyLab"
expect_ok "extension profile without sandbox or network.client" \
    "$verify" --decoded-profile "$root/good-extension.plist" \
    --bundle-id com.proxylab.ProxyLab.TransparentProxy --team AB12CD34EF --kind extension

echo "The explicit app group is accepted alongside the team wildcard..."
extension_profile "$root/group-and-wildcard.plist" "com.proxylab.ProxyLab.TransparentProxy" \
    "AB12CD34EF" "group.com.proxylab.ProxyLab" "app-proxy-provider" "" \
    "            <string>AB12CD34EF.*</string>"
expect_ok "explicit group plus team wildcard" \
    "$verify" --decoded-profile "$root/group-and-wildcard.plist" \
    --bundle-id com.proxylab.ProxyLab.TransparentProxy --team AB12CD34EF --kind extension

echo "An extension profile that also lists App Sandbox and network client is accepted..."
extension_profile "$root/with-sandbox.plist" "com.proxylab.ProxyLab.TransparentProxy" \
    "AB12CD34EF" "group.com.proxylab.ProxyLab" "app-proxy-provider" "include-sandbox"
expect_ok "extension profile with unrestricted sandbox keys" \
    "$verify" --decoded-profile "$root/with-sandbox.plist" \
    --bundle-id com.proxylab.ProxyLab.TransparentProxy --team AB12CD34EF --kind extension

echo "The extension entitlements file still sandboxes the provider..."
extension_entitlements="$resources/TransparentProxy/TransparentProxy.entitlements"
if [[ "$(plutil -extract 'com\.apple\.security\.app-sandbox' raw -o - -- "$extension_entitlements" 2>/dev/null || true)" != "true" ]]; then
    fail "TransparentProxy.entitlements must set com.apple.security.app-sandbox"
fi
if [[ "$(plutil -extract 'com\.apple\.security\.network\.client' raw -o - -- "$extension_entitlements" 2>/dev/null || true)" != "true" ]]; then
    fail "TransparentProxy.entitlements must set com.apple.security.network.client"
fi
if [[ "$(plutil -extract 'com\.apple\.security\.application-groups' json -o - -- "$extension_entitlements" 2>/dev/null || true)" != '["group.__APP_BUNDLE_ID__"]' ]]; then
    fail "TransparentProxy.entitlements must request only group.__APP_BUNDLE_ID__"
fi
mach_template="$(plutil -extract 'NetworkExtension.NEMachServiceName' raw -o - -- "$resources/TransparentProxy/Info.plist" 2>/dev/null || true)"
if [[ "$mach_template" != "group.__APP_BUNDLE_ID__.TransparentProxy" ]]; then
    fail "NEMachServiceName is '$mach_template', not group.__APP_BUNDLE_ID__.TransparentProxy"
fi

echo "The extension suffix matches the app..."
grep -q 'extensionSuffix = ".TransparentProxy"' \
    "$package_directory/Sources/ProxyLabCore/NetworkExtension/SystemTransparentProxyControl.swift" ||
    fail "SystemTransparentProxyControl.extensionSuffix is no longer .TransparentProxy"

echo "An ad-hoc bundle with one extension verifies..."
fake="$root/ProxyLab.app"
build_fake_app "$fake"
expect_ok "ad-hoc bundle" "$verify" "$fake"

echo "A bundle without the proxy engine does not verify..."
missing_engine="$root/missing-engine/ProxyLab.app"
mkdir -p "$root/missing-engine"
cp -R "$fake" "$missing_engine"
rm -f "$missing_engine/Contents/MacOS/proxy-engine"
expect_message "missing proxy-engine" "missing Contents/MacOS/proxy-engine" \
    "$verify" "$missing_engine"

echo "A certificate signature is not reported as unsigned..."
signing_keychain="$root/packaging.keychain-db"
cat >"$root/signing-openssl.cnf" <<'EOF'
[req]
distinguished_name = dn
x509_extensions = v3
prompt = no
[dn]
CN = Mac Developer: ProxyLab Packaging (4WRGLCZ743)
OU = AB12CD34EF
O = ProxyLab Test
C = US
[v3]
keyUsage = digitalSignature
extendedKeyUsage = codeSigning
EOF
openssl req -new -x509 -days 2 -nodes -config "$root/signing-openssl.cnf" \
    -keyout "$root/signing-key.pem" -out "$root/signing-cert.pem" >/dev/null 2>&1
openssl pkcs12 -export -inkey "$root/signing-key.pem" -in "$root/signing-cert.pem" \
    -out "$root/signing-id.p12" -passout pass:test
security create-keychain -p test "$signing_keychain"
security set-keychain-settings "$signing_keychain"
security unlock-keychain -p test "$signing_keychain"
security import "$root/signing-id.p12" -k "$signing_keychain" -P test -A
signed_app="$root/certificate-signed/ProxyLab.app"
build_fake_app "$signed_app"
signed_extension="$signed_app/Contents/Library/SystemExtensions/com.proxylab.ProxyLab.TransparentProxy.systemextension"
signing_identity="Mac Developer: ProxyLab Packaging (4WRGLCZ743)"
codesign --force --sign "$signing_identity" --keychain "$signing_keychain" \
    "$signed_app/Contents/MacOS/proxy-engine" >/dev/null
codesign --force --sign "$signing_identity" --keychain "$signing_keychain" "$signed_extension" >/dev/null
codesign --force --sign "$signing_identity" --keychain "$signing_keychain" "$signed_app" >/dev/null
expect_message "certificate signature recognized" "no signed entitlements" \
    "$verify" "$signed_app"

echo "Signed entitlements are read back as a property list..."
sed -e 's/__TEAM_ID__/AB12CD34EF/g' -e 's/__APP_BUNDLE_ID__/com.proxylab.ProxyLab/g' \
    "$resources/ProxyLab.entitlements" >"$root/app-entitlements.plist"
sed -e 's/__TEAM_ID__/AB12CD34EF/g' -e 's/__APP_BUNDLE_ID__/com.proxylab.ProxyLab/g' \
    "$resources/TransparentProxy/TransparentProxy.entitlements" >"$root/extension-entitlements.plist"
codesign --force --options runtime --timestamp=none \
    --sign "$signing_identity" --keychain "$signing_keychain" \
    "$signed_app/Contents/MacOS/proxy-engine" >/dev/null
codesign --force --options runtime --timestamp=none \
    --sign "$signing_identity" --keychain "$signing_keychain" \
    --entitlements "$root/extension-entitlements.plist" "$signed_extension" >/dev/null
codesign --force --options runtime --timestamp=none \
    --sign "$signing_identity" --keychain "$signing_keychain" \
    --entitlements "$root/app-entitlements.plist" "$signed_app" >/dev/null
expect_message "embedded entitlements parsed" "no embedded provisioning profile" \
    "$verify" "$signed_app"

echo "A complete development signature verifies, including cleanup..."
complete_app="$root/complete/ProxyLab.app"
build_fake_app "$complete_app"
complete_extension="$complete_app/Contents/Library/SystemExtensions/com.proxylab.ProxyLab.TransparentProxy.systemextension"
app_profile "$root/embedded-app.plist" "com.proxylab.ProxyLab" "AB12CD34EF" \
    "app-proxy-provider" true "2099-01-01T00:00:00Z" "OSX"
extension_profile "$root/embedded-extension.plist" "com.proxylab.ProxyLab.TransparentProxy" \
    "AB12CD34EF" "group.com.proxylab.ProxyLab"
openssl cms -sign -nodetach -binary -signer "$root/signing-cert.pem" -inkey "$root/signing-key.pem" \
    -in "$root/embedded-app.plist" -outform DER -out "$complete_app/Contents/embedded.provisionprofile"
openssl cms -sign -nodetach -binary -signer "$root/signing-cert.pem" -inkey "$root/signing-key.pem" \
    -in "$root/embedded-extension.plist" -outform DER \
    -out "$complete_extension/Contents/embedded.provisionprofile"
codesign --force --options runtime --timestamp=none \
    --sign "$signing_identity" --keychain "$signing_keychain" \
    "$complete_app/Contents/MacOS/proxy-engine" >/dev/null
codesign --force --options runtime --timestamp=none \
    --sign "$signing_identity" --keychain "$signing_keychain" \
    --entitlements "$root/extension-entitlements.plist" "$complete_extension" >/dev/null
codesign --force --options runtime --timestamp=none \
    --sign "$signing_identity" --keychain "$signing_keychain" \
    --entitlements "$root/app-entitlements.plist" "$complete_app" >/dev/null
expect_ok "development signature verifies" "$verify" "$complete_app"
if ! grep -q "development signature, extension entitlements present" "$root/out"; then
    fail "development verification did not report success"
    sed 's/^/    /' "$root/out" >&2
fi
if grep -q "unbound variable" "$root/err"; then
    fail "verification cleanup referenced an unbound variable"
    sed 's/^/    /' "$root/err" >&2
fi
security delete-keychain "$signing_keychain" >/dev/null 2>&1 || true
signing_keychain=""

echo "A second system extension is a stale bundle..."
mkdir -p "$fake/Contents/Library/SystemExtensions/Stale.systemextension"
expect_message "stale extension" "stale extension" "$verify" "$fake"
rm -rf "$fake/Contents/Library/SystemExtensions/Stale.systemextension"

echo "A bundle with the extension removed does not verify..."
rm -rf "$fake/Contents/Library/SystemExtensions"
expect_message "missing extension" "no Network Extension bundle" "$verify" "$fake"

echo "The developer stop script does not name forbidden commands..."
stop_script="$script_directory/stop-proxylab.sh"
[[ -x "$stop_script" ]] || fail "stop-proxylab.sh is not executable"
if grep -E -n '^[^#]*\b(pkill|killall|networksetup|systemextensionsctl)\b|^[^#]*zshrc' "$stop_script"; then
    fail "stop-proxylab.sh contains a forbidden command"
fi

echo "The developer stop script detects a stale 127.0.0.1:9090 proxy..."
# shellcheck disable=SC1090
source "$stop_script"
stale_proxy='
  HTTPEnable : 1
  HTTPPort : 9090
  HTTPProxy : 127.0.0.1
  HTTPSEnable : 1
  HTTPSPort : 9090
  HTTPSProxy : 127.0.0.1
'
scoped_proxy='
  HTTPEnable : 0
  HTTPSEnable : 0
  __SCOPED__ : <dictionary> {
    en0 : <dictionary> {
      HTTPEnable : 1
      HTTPProxy : 127.0.0.1
      HTTPPort : 9090
      HTTPSEnable : 1
      HTTPSProxy : 127.0.0.1
      HTTPSPort : 9090
    }
  }
'
clear_proxy='
  HTTPEnable : 0
  HTTPSEnable : 0
'
other_proxy='
  HTTPEnable : 1
  HTTPPort : 8080
  HTTPProxy : proxy.example
  HTTPSEnable : 1
  HTTPSPort : 8080
  HTTPSProxy : proxy.example
'
[[ "$(classify_proxy_text "$stale_proxy")" == "stale" ]] || fail "owned HTTP and HTTPS proxy was not detected"
[[ "$(classify_proxy_text "$scoped_proxy")" == "stale" ]] || fail "scoped 127.0.0.1:9090 proxy was not detected"
[[ "$(classify_proxy_text "$clear_proxy")" == "clear" ]] || fail "a disabled proxy was reported as stale"
[[ "$(classify_proxy_text "$other_proxy")" == "clear" ]] || fail "another proxy was reported as ProxyLab's"
[[ "$(classify_proxy_text "")" == "unreadable" ]] || fail "empty proxy output was treated as clear"
if grep -q "ProxyLab process is stopped but macOS is still configured to use ProxyLab's proxy." "$stop_script"; then
    :
else
    fail "stop-proxylab.sh does not report a stopped process with a stale proxy"
fi

echo "Passing the .app path to the build script is refused..."
expect_message "app path argument" "not the .app itself" \
    env -u PROXYLAB_TEAM_ID -u PROXYLAB_SIGNING_IDENTITY \
    -u PROXYLAB_APP_PROFILE -u PROXYLAB_EXTENSION_PROFILE \
    "$make_bundle" "$root/ProxyLab.app"

if [[ "$failures" -ne 0 ]]; then
    echo "$failures packaging check(s) failed" >&2
    exit 1
fi

echo "Packaging checks passed."
