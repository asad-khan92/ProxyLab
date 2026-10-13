#!/bin/bash
#
# Stops a development ProxyLab.app and checks that its bundled proxy engine
# no longer owns ports 9090 and 9091.
#
# Stopping the process is not enough. ProxyLab may have pointed this Mac's
# HTTP and HTTPS proxy at 127.0.0.1:9090. This script reads that live route
# with `scutil --proxy` and will not signal the app or the bundled engine
# while the route still points there, or while the route cannot be read.
# It does not change proxy settings: it does not know the saved baseline,
# and writing them may need an administrator. Open ProxyLab and quit it
# from the app; that is the restore.
#
# This is for a developer. The app's own Start/Stop control and Command-Q are
# what a normal launch uses.
#
# What it will signal, and only after the live route is not 127.0.0.1:9090:
#   - ProxyLab.app, by asking the application to quit
#   - if that app is still running afterwards, that app executable only
#   - a proxy-engine whose path is Contents/MacOS/proxy-engine inside a
#     ProxyLab.app bundle, and only when it is still listening afterwards
#
# What it will not signal: any other process, including a proxy-engine built
# by Cargo, and including any other proxy. It does not remove a system
# extension or edit a shell configuration file.

set -euo pipefail

ports=(9090 9091)
owned_host="127.0.0.1"
owned_port="9090"

die() {
    echo "error: $*" >&2
    exit 1
}

# Reads `scutil --proxy` text on stdin. Exits 0 when HTTP or HTTPS is enabled
# at 127.0.0.1:9090, including a per-interface block. Exits 1 otherwise.
proxy_text_is_stale() {
    awk -v host="$owned_host" -v port="$owned_port" '
        function consider() {
            if (http_enable == "1" && tolower(http_proxy) == host && http_port == port) stale = 1
            if (https_enable == "1" && tolower(https_proxy) == host && https_port == port) stale = 1
        }
        function reset_http() { http_enable = ""; http_proxy = ""; http_port = "" }
        function reset_https() { https_enable = ""; https_proxy = ""; https_port = "" }
        BEGIN { reset_http(); reset_https(); stale = 0 }
        {
            line = $0
            gsub(/^[ \t]+/, "", line)
            split(line, parts, " : ")
            key = parts[1]
            value = parts[2]
            gsub(/[ \t]+$/, "", value)
            if (key == "HTTPEnable") { consider(); reset_http(); http_enable = value; next }
            if (key == "HTTPProxy") { http_proxy = value; next }
            if (key == "HTTPPort") { http_port = value; next }
            if (key == "HTTPSEnable") { consider(); reset_https(); https_enable = value; next }
            if (key == "HTTPSProxy") { https_proxy = value; next }
            if (key == "HTTPSPort") { https_port = value; next }
            if (key == "}") { consider(); reset_http(); reset_https(); next }
        }
        END { consider(); exit stale ? 0 : 1 }
    '
}

# stale, clear, or unreadable. Empty or failed output is unreadable, not clear.
classify_proxy_text() {
    local text="$1"
    if [[ -z "${text//[[:space:]]/}" ]]; then
        printf 'unreadable\n'
        return
    fi
    if printf '%s\n' "$text" | proxy_text_is_stale; then
        printf 'stale\n'
    else
        printf 'clear\n'
    fi
}

read_proxy_text() {
    scutil --proxy 2>/dev/null || true
}

# Full executable path for one pid, or empty when it cannot be read.
executable_path() {
    local pid="$1"
    lsof -a -p "$pid" -d txt -Fn 2>/dev/null | awk '/^n/ { print substr($0, 2); exit }'
}

is_proxylab_app() {
    [[ "$1" == *"/ProxyLab.app/Contents/MacOS/ProxyLab" ]]
}

is_bundled_engine() {
    [[ "$1" == *"/ProxyLab.app/Contents/MacOS/proxy-engine" ]]
}

app_pids() {
    local pid path
    while read -r pid; do
        [[ -n "$pid" ]] || continue
        path="$(executable_path "$pid")"
        if is_proxylab_app "$path"; then
            printf '%s\n' "$pid"
        fi
    done < <(pgrep -f '/ProxyLab\.app/Contents/MacOS/ProxyLab' || true)
}

listener_pids() {
    local port="$1"
    lsof -nP -iTCP:"$port" -sTCP:LISTEN -t 2>/dev/null || true
}

bundled_engine_listening() {
    local port pid path
    for port in "${ports[@]}"; do
        while read -r pid; do
            [[ -n "$pid" ]] || continue
            path="$(executable_path "$pid")"
            if is_bundled_engine "$path"; then
                return 0
            fi
        done < <(listener_pids "$port")
    done
    return 1
}

# SIGTERM, then SIGKILL, for one pid this script has already identified.
# Only called after the live route is clear.
signal_pid() {
    local pid="$1" path
    path="$(executable_path "$pid")"
    if ! is_proxylab_app "$path" && ! is_bundled_engine "$path"; then
        echo "refusing to signal pid $pid ($path)"
        return 0
    fi
    echo "signaling pid $pid ($path)"
    kill -TERM "$pid" 2>/dev/null || true
    local attempt
    for attempt in 1 2 3 4 5; do
        if ! kill -0 "$pid" 2>/dev/null; then
            return 0
        fi
        sleep 0.2
    done
    if kill -0 "$pid" 2>/dev/null; then
        kill -KILL "$pid" 2>/dev/null || true
    fi
}

report_stale_proxy() {
    local state="$1"
    echo
    if [[ "$state" == "unreadable" ]]; then
        echo "ProxyLab could not read this Mac's effective HTTP and HTTPS proxy."
        echo "Processes were left running. A proxy setting of 127.0.0.1:9090 cannot be ruled out."
    elif [[ -z "$(app_pids)" ]] && ! bundled_engine_listening; then
        echo "ProxyLab process is stopped but macOS is still configured to use ProxyLab's proxy."
        echo "Open ProxyLab and quit it from the app so it can restore the saved proxy."
        echo "This script does not change proxy settings."
    else
        echo "macOS is still sending HTTP or HTTPS to ProxyLab at 127.0.0.1:9090."
        echo "ProxyLab was left running so that route is not pointed at a stopped proxy."
        echo "Approve the restore prompt, or use Stop in ProxyLab, then quit the app."
        echo "This script does not change proxy settings."
    fi
}

report_listeners() {
    local port pid path any status=0
    echo
    echo "Remaining listeners:"
    for port in "${ports[@]}"; do
        any=""
        while read -r pid; do
            [[ -n "$pid" ]] || continue
            any=yes
            path="$(executable_path "$pid")"
            if [[ -z "$path" ]]; then
                path="$(ps -p "$pid" -o comm= 2>/dev/null | sed 's/^ *//' || true)"
            fi
            if is_bundled_engine "$path"; then
                echo "  $port: still owned by ProxyLab's bundled proxy-engine (pid $pid, $path)"
                status=1
            else
                echo "  $port: pid $pid ($path) — left running"
            fi
        done < <(listener_pids "$port")
        if [[ -z "$any" ]]; then
            echo "  $port: free"
        fi
    done
    if [[ -n "$(app_pids)" ]]; then
        echo "ProxyLab.app is still running."
        status=1
    else
        echo "ProxyLab.app is not running."
    fi
    return "$status"
}

main() {
    command -v lsof >/dev/null 2>&1 || die "lsof is required to see who owns ports 9090 and 9091"
    command -v scutil >/dev/null 2>&1 || die "scutil is required to read the effective proxy"

    echo "Asking ProxyLab to quit..."
    osascript -e 'tell application "ProxyLab" to quit' >/dev/null 2>&1 || true

    # The quit above asks the app to restore the proxy before it exits.
    # That can wait on an administrator dialog. Killing the process here
    # is what leaves HTTP and HTTPS pointed at a proxy that is gone.
    # An app that is already gone is not given that wait: the route is
    # classified immediately.
    local attempt=0 state
    state="$(classify_proxy_text "$(read_proxy_text)")"
    if [[ "$state" != "clear" && -n "$(app_pids)" ]]; then
        while [[ "$attempt" -lt 240 ]]; do
            state="$(classify_proxy_text "$(read_proxy_text)")"
            if [[ "$state" == "clear" || -z "$(app_pids)" ]]; then
                break
            fi
            sleep 0.25
            attempt=$((attempt + 1))
        done
        state="$(classify_proxy_text "$(read_proxy_text)")"
    fi
    if [[ "$state" != "clear" ]]; then
        report_stale_proxy "$state"
        report_listeners || true
        exit 1
    fi

    while read -r pid; do
        [[ -n "$pid" ]] || continue
        signal_pid "$pid"
    done < <(app_pids)

    # A bundled engine still listening after the app was asked to quit is the
    # child that shutdown should have stopped. Only that path is signaled,
    # and only because the live route is no longer the owned proxy.
    local port
    for port in "${ports[@]}"; do
        while read -r pid; do
            [[ -n "$pid" ]] || continue
            path="$(executable_path "$pid")"
            if is_bundled_engine "$path"; then
                signal_pid "$pid"
            fi
        done < <(listener_pids "$port")
    done

    if report_listeners; then
        exit 0
    fi
    exit 1
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
