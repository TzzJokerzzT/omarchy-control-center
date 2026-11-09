import QtQuick
import Quickshell
import Quickshell.Io
import qs.Ui

// Bar button for Omarchy Control Center.
//
// Two behaviours, decided by one fact: whether the binary is on PATH.
//   installed      -> open the inventory in a terminal, reusing the window if there is one
//   not installed  -> open the installation instructions instead of failing silently
//
// Every command is a fixed argv array. Nothing from the environment, a file or the
// network is interpolated into a shell string, which is the rule Omarchy's own plugin
// guide sets for code that runs unsandboxed inside the shell.
BarWidget {
  id: root
  moduleName: "tzzjokerzzt.omarchy-control-center"

  property bool installed: false
  property bool probed: false

  Process {
    id: probe
    // A fixed command: `command -v` needs a shell, but nothing variable is passed to it.
    command: ["bash", "-lc", "command -v omarchy-control-center"]
    running: true
    onExited: function (exitCode) {
      root.installed = exitCode === 0
      root.probed = true
    }
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: "\uDB80\uDC7E"
    onPressed: function (button) {
      if (!root.probed) return
      if (root.installed) {
        if (button === Qt.RightButton) {
          // Reuse an existing window when one is open.
          Quickshell.execDetached(["omarchy-launch-or-focus-tui", "omarchy-control-center"])
        } else {
          Quickshell.execDetached(["omarchy-launch-tui", "omarchy-control-center"])
        }
      } else {
        Quickshell.execDetached([
          "omarchy-launch-browser",
          "https://github.com/TzzJokerzzT/omarchy-control-center#build-and-run"
        ])
      }
    }
  }
}
