import QtQuick
import Quickshell
import qs.Ui
import qs.Commons

// A button on the bar: left click opens the picker, right click its settings.
// The three options in Omarchy's widget settings are forwarded to omawall.
BarWidget {
  id: root
  moduleName: "kirayoshikage7777.omawall"

  readonly property string binary: Quickshell.env("HOME") + "/.local/share/omawall/omawall"
  function run(args) { Quickshell.execDetached([root.binary].concat(args)) }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  // Settings are pushed only when they change here, so values changed inside
  // omawall's own settings are not overwritten on every shell start.
  property var applied: ({})
  property bool ready: false
  function push() {
    var want = {
      saver: String(setting("powerSaver", "auto")),
      apply_on_close: setting("keepOnClose", false) === true ? "true" : "false",
      live_preview: setting("livePreview", true) === false ? "false" : "true"
    }
    for (var k in want) {
      if (ready && applied[k] !== want[k]) root.run(["set", k, want[k]])
    }
    applied = want
    ready = true
  }
  onSettingsChanged: push()
  Component.onCompleted: push()

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: String.fromCodePoint(0xF02F9)
    tooltipText: "omawall"
    onPressed: function(b) {
      if (b === Qt.RightButton) root.run(["settings"])
      else root.run(["toggle"])
    }
  }
}
