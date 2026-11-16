#!/bin/bash
# Стенд: отдельный невидимый sway, в нём демон omawall со своими настройками и сокетом.
T=${OMAWALL_TEST_DIR:-${XDG_RUNTIME_DIR:?}/omawall-test}
BIN=${BIN:-$HOME/projects/omawall/target/debug/omawall}
mkdir -p "$T"/{config,state,cache}
envs() {
  export WAYLAND_DISPLAY=$(cat "$T/display" 2>/dev/null) SWAYSOCK=$(cat "$T/swaysock" 2>/dev/null)
  export OMAWALL_SOCK="$T/omawall.sock" XDG_CONFIG_HOME="$T/config" XDG_CACHE_HOME="$T/cache"
  [[ -n $REAL_STATE ]] || export XDG_STATE_HOME="$T/state"
  unset HYPRLAND_INSTANCE_SIGNATURE DISPLAY
}
case $1 in
  start)
    rm -f "$T/display" "$T/swaysock"
    cat > "$T/sway.conf" <<C
output HEADLESS-1 resolution ${RES:-1920x1080} scale ${SCALE:-1}
exec sh -c 'echo \$WAYLAND_DISPLAY > $T/display; echo \$SWAYSOCK > $T/swaysock'
C
    ( unset WAYLAND_DISPLAY DISPLAY HYPRLAND_INSTANCE_SIGNATURE SWAYSOCK; WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 setsid -f sway --unsupported-gpu -c "$T/sway.conf" >"$T/sway.log" 2>&1 )
    for i in $(seq 50); do [[ -s $T/display ]] && break; sleep 0.1; done
    envs
    mkdir -p "$XDG_STATE_HOME/omarchy/current"
    setsid -f "$BIN" daemon >"$T/daemon.log" 2>&1
    sleep 0.6; "$BIN" status ;;
  stop) envs; "$BIN" quit 2>/dev/null; swaymsg exit >/dev/null 2>&1; sleep 0.2 ;;
  restart-daemon) envs; "$BIN" quit 2>/dev/null; sleep 0.3; setsid -f "$BIN" daemon >"$T/daemon.log" 2>&1; sleep 0.6; "$BIN" status ;;
  shot) envs; grim "${2:-$T/shot.png}" ;;
  log) tail -${2:-30} "$T/daemon.log" ;;
  sway) envs; shift; swaymsg "$@" ;;
  *) envs; "$BIN" "$@" ;;
esac
