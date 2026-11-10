#!/usr/bin/env bash
set -u

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BIN=${BIN:-${CARGO_TARGET_DIR:-$ROOT/target}/debug/gnome-foreground-booster}
WORK=$(mktemp -d)
STATE_DIR=${XDG_RUNTIME_DIR:?}/gnome-foreground-booster
FAILED=0
DAEMON=
BRIDGE=

weight() { systemctl --user show "$1" -p CPUWeight --value; }

expect_weight() {
    local unit=$1 want=$2 got= i
    for i in $(seq 1 30); do
        got=$(weight "$unit")
        [ "$got" = "$want" ] && { echo "ok   $3"; return; }
        sleep 0.1
    done
    echo "FAIL $3 ($unit: wanted '$want', got '$got')"
    FAILED=1
}

start_daemon() {
    RUST_LOG=gnome_foreground_booster=debug "$BIN" >"$WORK/daemon.log" 2>&1 &
    DAEMON=$!
    sleep 0.5
}

cleanup() {
    [ -n "$DAEMON" ] && kill "$DAEMON" 2>/dev/null
    [ -n "$BRIDGE" ] && kill "$BRIDGE" 2>/dev/null
    exec 3>&- 2>/dev/null
    systemctl --user stop gfb-e2e-a gfb-e2e-b 2>/dev/null
    rm -rf "$WORK" "$STATE_DIR/state"
}
trap cleanup EXIT

[ -x "$BIN" ] || { echo "binary not found: $BIN (build it first, or set BIN)"; exit 2; }

if busctl --user status org.luminusos.ForegroundBooster >/dev/null 2>&1; then
    echo "a gnome-foreground-booster is already running; stop it first"
    exit 2
fi

systemd-run --user --quiet --unit=gfb-e2e-a sleep 300
systemd-run --user --quiet --unit=gfb-e2e-b sleep 300
sleep 0.3
PA=$(systemctl --user show gfb-e2e-a -p MainPID --value)
PB=$(systemctl --user show gfb-e2e-b -p MainPID --value)

mkfifo "$WORK/focus"
exec 3<>"$WORK/focus"
gjs "$ROOT/tests/e2e/fake-bridge.js" <&3 >"$WORK/bridge.log" 2>&1 &
BRIDGE=$!
sleep 1

start_daemon

echo $PA >&3
expect_weight gfb-e2e-a 400 "focusing a boosts a"
expect_weight gfb-e2e-b "[not set]" "focusing a leaves b alone"

echo $PB >&3
expect_weight gfb-e2e-a "[not set]" "moving focus to b restores a"
expect_weight gfb-e2e-b 400 "moving focus to b boosts b"

echo 0 >&3
expect_weight gfb-e2e-b "[not set]" "no focus releases b"

echo $PA >&3
expect_weight gfb-e2e-a 400 "refocusing a boosts a"
kill -9 "$BRIDGE"; wait "$BRIDGE" 2>/dev/null; BRIDGE=
expect_weight gfb-e2e-a "[not set]" "bridge leaving the bus releases the boost"

gjs "$ROOT/tests/e2e/fake-bridge.js" <&3 >"$WORK/bridge.log" 2>&1 &
BRIDGE=$!
sleep 1
echo $PA >&3
expect_weight gfb-e2e-a 400 "boost resumes when the bridge returns"

kill -9 "$DAEMON"; wait "$DAEMON" 2>/dev/null; DAEMON=
expect_weight gfb-e2e-a 400 "a crashed daemon leaves the boost behind"
echo 0 >&3
sleep 0.3
start_daemon
expect_weight gfb-e2e-a "[not set]" "a restarted daemon restores the stale boost"

echo $PA >&3
expect_weight gfb-e2e-a 400 "boost works after recovery"
kill -TERM "$DAEMON"; wait "$DAEMON" 2>/dev/null; DAEMON=
expect_weight gfb-e2e-a "[not set]" "SIGTERM restores the boost"

[ "$FAILED" = 0 ] && echo "all e2e checks passed" || { echo; cat "$WORK/daemon.log"; exit 1; }
