#!/usr/bin/env bash
set -u

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BIN=${BIN:-${CARGO_TARGET_DIR:-$ROOT/target}/debug/gnome-foreground-booster}
UID_=$(id -u)
SLICE_DIR=/sys/fs/cgroup/user.slice/user-$UID_.slice/user@$UID_.service/app.slice/app-gfbe2e.slice
WORK=$(mktemp -d)
STATE_DIR=${XDG_RUNTIME_DIR:?}/gnome-foreground-booster
FAILED=0
DAEMON=

weight() { systemctl --user show "$1" -p CPUWeight --value; }

# Mimic Mutter: -1 on the focused unit, a timestamp on the one that lost focus.
focus() { python3 -c 'import os,sys; os.setxattr(sys.argv[1], "user.xdg.inactive-since", sys.argv[2].encode())' "$1" "$2"; }

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
    RUST_LOG=gnome_foreground_booster=debug "$BIN" --app-slice "$SLICE_DIR" >>"$WORK/daemon.log" 2>&1 &
    DAEMON=$!
    sleep 0.5
}

cleanup() {
    [ -n "$DAEMON" ] && kill "$DAEMON" 2>/dev/null && wait "$DAEMON" 2>/dev/null
    systemctl --user stop gfb-e2e-a gfb-e2e-b gfb-e2e-c app-gfbe2e-nested.slice app-gfbe2e.slice 2>/dev/null
    rm -rf "$WORK" "$STATE_DIR/state"
}
trap cleanup EXIT

[ -x "$BIN" ] || { echo "binary not found: $BIN (build it first, or set BIN)"; exit 2; }

if busctl --user status org.luminusos.ForegroundBooster >/dev/null 2>&1; then
    echo "a gnome-foreground-booster is already running; stop it first"
    exit 2
fi

systemd-run --user --quiet --slice=app-gfbe2e.slice --unit=gfb-e2e-a sleep 300
systemd-run --user --quiet --slice=app-gfbe2e.slice --unit=gfb-e2e-b sleep 300
sleep 0.3
A=$SLICE_DIR/gfb-e2e-a.service
B=$SLICE_DIR/gfb-e2e-b.service
focus "$A" -1

start_daemon
expect_weight gfb-e2e-a 400 "the unit focused at startup is boosted"

focus "$A" 1000
focus "$B" -1
expect_weight gfb-e2e-a "[not set]" "moving focus to b restores a"
expect_weight gfb-e2e-b 400 "moving focus to b boosts b"

focus "$B" 2000
expect_weight gfb-e2e-b "[not set]" "no focused unit releases b"

systemd-run --user --quiet --slice=app-gfbe2e-nested.slice --unit=gfb-e2e-c sleep 300
sleep 0.3
focus "$SLICE_DIR/app-gfbe2e-nested.slice/gfb-e2e-c.service" -1
expect_weight app-gfbe2e-nested.slice 400 "a unit created later in a nested slice boosts that slice"
focus "$SLICE_DIR/app-gfbe2e-nested.slice/gfb-e2e-c.service" 3000
expect_weight app-gfbe2e-nested.slice "[not set]" "losing focus restores the nested slice"

focus "$A" -1
expect_weight gfb-e2e-a 400 "refocusing a boosts a"
kill -9 "$DAEMON"; wait "$DAEMON" 2>/dev/null; DAEMON=
expect_weight gfb-e2e-a 400 "a crashed daemon leaves the boost behind"
focus "$A" 4000
start_daemon
expect_weight gfb-e2e-a "[not set]" "a restarted daemon restores the stale boost"

focus "$A" -1
expect_weight gfb-e2e-a 400 "boost works after recovery"
kill -TERM "$DAEMON"; wait "$DAEMON" 2>/dev/null; DAEMON=
expect_weight gfb-e2e-a "[not set]" "SIGTERM restores the boost"

if [ "$FAILED" = 0 ]; then
    echo "all e2e checks passed"
else
    echo
    cat "$WORK/daemon.log"
    exit 1
fi
