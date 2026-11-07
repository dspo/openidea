#!/bin/bash
# Gate-2 diagnostic probe: the IME full chain in a private session
# (dbus-run-session + headless sway + fcitx5 via input-method-v2).
#
# NOT a verify leg yet — this is the harness that produced the LINUX-GATE-REPORT
# §2 evidence (composition never engages + double-insert). Run from the app
# directory:
#
#   dbus-run-session -- bash script/ime-probe.sh
#
# Screenshots and logs land under /tmp/takeover/ime/. The user desktop's bus
# and ibus daemon are never touched: everything lives in the private session.
set -u
cd "$(dirname "$0")/.."

export XDG_CONFIG_HOME=/tmp/takeover/ime/config
export XDG_CACHE_HOME=/tmp/takeover/ime/cache
export XDG_DATA_HOME=/tmp/takeover/ime/data
mkdir -p "$XDG_CONFIG_HOME/fcitx5" "$XDG_CACHE_HOME" "$XDG_DATA_HOME" /tmp/takeover/ime

cat > "$XDG_CONFIG_HOME/fcitx5/profile" <<'EOF'
[Groups/0]
Name=Default
Default Layout=us
DefaultIM=pinyin

[Groups/0/Items/0]
Name=keyboard-us
Layout=

[Groups/0/Items/1]
Name=pinyin
Layout=

[GroupOrder]
0=Default
EOF

RUNTIME=/run/user/$(id -u)
BEFORE=$(ls "$RUNTIME")

WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 sway -d -c /tmp/takeover/verify/probe-sway-config >/tmp/takeover/ime/sway-dbg.log 2>&1 &
SWAY=$!
WL=""
for i in $(seq 1 50); do
  NEW=$(comm -13 <(echo "$BEFORE" | sort) <(ls "$RUNTIME" | sort) | grep -E '^wayland-[0-9]+$' | sort | tail -1)
  [ -n "$NEW" ] && WL="$NEW" && break
  sleep 0.2
done
[ -z "$WL" ] && { echo "NO SWAY SOCKET"; kill $SWAY; exit 1; }
export WAYLAND_DISPLAY=$WL
echo "sway socket: $WL"

wtype -s 2147483647 >/dev/null 2>&1 &
HOLDER=$!
sleep 0.5

fcitx5 --verbose "key_trace=5" >/tmp/takeover/ime/fcitx5.log 2>&1 &
FCITX=$!
sleep 2.5

OPENIDEA_GUI_PROJECT=/home/dspo/projects/dspo/openidea/apps/openidea-gui \
OPENIDEA_GUI_OPEN=src/main.rs \
OPENIDEA_GUI_SETTINGS_DIR=/tmp/takeover/ime/settings \
RUST_LOG=info \
./target/debug/openidea-gui >/tmp/takeover/ime/app.log 2>&1 &
APP=$!
sleep 7

swaymsg seat seat0 cursor move warp 700 300 >/dev/null
wlrctl pointer click left >/dev/null 2>&1
sleep 0.5

echo "== state before trigger: $(fcitx5-remote 2>&1) im=$(fcitx5-remote -n 2>&1)"
wtype -M ctrl -P space -p space -m ctrl
sleep 0.8
echo "== state after ctrl+space: $(fcitx5-remote 2>&1) im=$(fcitx5-remote -n 2>&1)"

# dbus toggle as the alternative channel
fcitx5-remote -t
sleep 0.5
echo "== state after dbus -t: $(fcitx5-remote 2>&1) im=$(fcitx5-remote -n 2>&1)"

wtype -s 80 n; wtype -s 80 i
sleep 1.2
grim /tmp/takeover/ime/v2-1-composing.png
echo "== app ime lines so far:"
grep "\[ime\]" /tmp/takeover/ime/app.log | tail -4

wtype -P space -p space
sleep 1.0
grim /tmp/takeover/ime/v2-2-committed.png

echo "== key_trace tail:"
grep -E "key_trace|Key|grab" /tmp/takeover/ime/fcitx5.log | tail -14

kill $APP $FCITX $HOLDER 2>/dev/null
sleep 1
kill $SWAY 2>/dev/null
sleep 1
echo "done"
