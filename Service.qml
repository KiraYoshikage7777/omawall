import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Hyprland

// The always-on half of omawall. The wallpaper itself is drawn by the omawall
// daemon (a systemd user service), not inside the shell: video decoding and
// GPU work stay out of the long-lived omarchy-shell process.
//
// Enabling the plugin installs and starts the daemon; disabling or removing it
// through Omarchy (menu or `omarchy plugin disable|remove`) stops it and puts
// Omarchy's own background back. A shell restart or plugin reload also tears
// this item down, so the helper first checks whether the plugin is really off.
Scope {
  id: root

  property var shell: null
  property var manifest: null

  // The shell strips __sourceDir from third-party manifests; this file's URL
  // is the reliable way to find the bundled binary.
  readonly property string sourceDir: {
    var url = String(Qt.resolvedUrl("."))
    if (url.indexOf("file://") !== 0) return ""
    try { url = decodeURIComponent(url.substring(7)) } catch (e) { return "" }
    return url.replace(/\/$/, "")
  }
  readonly property string helper: sourceDir + "/bin/omawall-plugin"
  readonly property string binary: Quickshell.env("HOME") + "/.local/share/omawall/omawall"

  function run(args) { Quickshell.execDetached([root.binary].concat(args)) }

  IpcHandler {
    target: "omawall"
    function toggle(): void { root.run(["toggle"]) }
    function show(): void { root.run(["show"]) }
    function hide(): void { root.run(["hide"]) }
    function settings(): void { root.run(["settings"]) }
  }

  property bool started: false

  // Super+Ctrl+Space and a layer rule are set at runtime through `hyprctl eval`,
  // never written into your Hyprland files. A config reload drops runtime
  // settings, so they are set again after each one; disabling the plugin
  // reloads the config, which brings Omarchy's own binding back.
  readonly property string hyprSetup:
      'o.rebind("SUPER + CTRL + SPACE", "Wallpaper picker (omawall)", "' + root.binary + ' toggle")\n'
    + 'hl.layer_rule({ match = { namespace = "^omawall" }, no_anim = true, animation = "none" })'
  function applyHyprland() {
    Quickshell.execDetached(["hyprctl", "eval", root.hyprSetup])
  }

  Connections {
    target: Hyprland
    function onRawEvent(event) {
      if (root.started && event && String(event.name) === "configreloaded") root.applyHyprland()
    }
  }

  Component.onCompleted: {
    if (sourceDir === "") return
    started = true
    Quickshell.execDetached(["bash", root.helper, "on", root.sourceDir])
    root.applyHyprland()
  }

  Component.onDestruction: {
    if (!started) return
    // The plugin folder may be deleted right after this (remove), so the
    // helper runs from a copy.
    // Omarchy moves the folder away before unloading on remove, so the helper
    // also keeps a copy of itself in ~/.local/share/omawall.
    Quickshell.execDetached(["sh", "-c",
      'dst="${XDG_RUNTIME_DIR:-/tmp}/omawall-plugin-off.sh"; src="$1"; [ -f "$src" ] || src="$HOME/.local/share/omawall/omawall-plugin"; cp -f "$src" "$dst" && exec bash "$dst" off-if-disabled',
      "sh", root.helper])
  }
}
