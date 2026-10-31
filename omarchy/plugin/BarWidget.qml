import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
import qs.Ui

// Stasis bar widget: a rest timer that drives `stasis` through its CLI.
//
// The engine (stasis daemon) owns the lock state; this widget only reads the
// status file it publishes and writes `rest.requested` events via
// `stasis rest <minutes>`.  That keeps one control path for the bar, aide,
// and the tray UI.
//
// While a rest is active the pointer and keyboard are grabbed by evdev, so
// the bar cannot be clicked until the scheduled unlock (or the j-j-j
// gesture).  The countdown is therefore read-only feedback.
BarWidget {
  id: root
  moduleName: "stasis.rest"

  readonly property string home: Quickshell.env("HOME")
  readonly property string stateHome: Quickshell.env("XDG_STATE_HOME") || (home + "/.local/state")
  readonly property string statusFile: {
    var configured = String(setting("statusFile", "") || "")
    return configured !== "" ? configured : root.stateHome + "/stasis/status.json"
  }
  readonly property string binary: {
    var configured = String(setting("binary", "~/.local/bin/stasis") || "")
    if (configured.indexOf("~/") === 0) return root.home + configured.slice(1)
    return configured
  }
  readonly property var durations: {
    var raw = setting("durations", "5,10,20,30")
    var parts = Array.isArray(raw) ? raw : String(raw).split(",")
    var out = []
    for (var i = 0; i < parts.length; i++) {
      var n = parseInt(String(parts[i]).trim(), 10)
      if (n > 0 && n <= 240) out.push(n)
    }
    return out.length > 0 ? out : [5, 10, 20, 30]
  }

  // Optional rest-break status (work time / fatigue).  rest-break writes it
  // once a minute; the first existing candidate wins so a fresh checkout and a
  // deployed copy both work without configuration.
  readonly property var restBreakCandidates: {
    var out = []
    var configured = String(setting("restBreakStatusFile", "") || "")
    if (configured !== "") out.push(configured)
    out.push(root.stateHome + "/rest-break/status.json")
    out.push(root.home + "/stasis/rest-break/status.json")
    out.push(root.home + "/stasis/rest-break/target/release/status.json")
    return out
  }
  property int restBreakIndex: 0
  property var restBreak: ({})
  property bool restBreakOnline: false

  readonly property double workMinutes: {
    var v = Number(root.restBreak ? root.restBreak.workMinutes : NaN)
    return isFinite(v) && v > 0 ? v : 0
  }
  readonly property double fatigueMinutes: {
    var v = Number(root.restBreak ? root.restBreak.fatigueMinutes : NaN)
    return isFinite(v) && v > 0 ? v : 0
  }
  readonly property int nextRestMinutes: {
    var v = Number(root.restBreak ? root.restBreak.nextRestMinutes : NaN)
    return isFinite(v) && v > 0 ? Math.round(v) : 0
  }
  readonly property string nextRestAt: {
    return String(root.restBreak && root.restBreak.nextRestAt ? root.restBreak.nextRestAt : "")
  }

  property var state: ({})
  property bool online: false
  property bool locked: false
  property string phase: ""
  property string unlockAt: ""
  property bool failed: false
  property double nowMs: Date.now()

  // Panel lifecycle.  `opened` / `open()` / `close()` are the contract the bar
  // uses to route popout ownership and hotkey summoning.
  property bool panelOpen: false
  readonly property bool opened: panelOpen
  function open() { panelOpen = true }
  function close() { panelOpen = false }
  function togglePanel() { panelOpen ? close() : open() }

  readonly property int remainingSeconds: secondsUntil(unlockAt)
  readonly property bool starting: online && !locked && !failed && phase === "waiting" && remainingSeconds > 0

  function secondsUntil(iso) {
    if (!iso) return 0
    var end = Date.parse(iso)
    if (isNaN(end)) return 0
    return Math.max(0, Math.round((end - root.nowMs) / 1000))
  }

  function formatMinutes(value) {
    var total = Math.max(0, Math.round(Number(value) || 0))
    if (total < 60) return total + "m"
    var h = Math.floor(total / 60)
    var m = total % 60
    return m === 0 ? h + "h" : h + "h" + m + "m"
  }

  function hhmm(iso) {
    var t = Date.parse(String(iso || ""))
    if (isNaN(t)) return ""
    var d = new Date(t)
    return (d.getHours() < 10 ? "0" : "") + d.getHours() + ":" +
           (d.getMinutes() < 10 ? "0" : "") + d.getMinutes()
  }

  function parseRestBreak(text) {
    var parsed = null
    try {
      parsed = JSON.parse(String(text || ""))
    } catch (e) {
      parsed = null
    }
    if (!parsed || typeof parsed !== "object" || typeof parsed.checkedAt !== "string") {
      root.restBreak = ({})
      root.restBreakOnline = false
      return
    }
    // rest-break runs once a minute; three minutes tolerates a missed tick
    // without showing yesterday's numbers as current.
    var checked = Date.parse(parsed.checkedAt)
    var age = isNaN(checked) ? Infinity : (Date.now() - checked)
    root.restBreak = parsed
    root.restBreakOnline = age >= -5000 && age <= 180000
  }

  function restBreakLoadFailed() {
    if (root.restBreakIndex + 1 < root.restBreakCandidates.length) {
      root.restBreakIndex = root.restBreakIndex + 1
    } else {
      root.restBreak = ({})
      root.restBreakOnline = false
    }
  }

  function clockText(seconds) {
    var m = Math.floor(seconds / 60)
    var s = seconds % 60
    return m + ":" + (s < 10 ? "0" : "") + s
  }

  readonly property string label: {
    if (root.online && root.locked) return clockText(remainingSeconds)
    if (root.starting) return "…"
    // Idle: how long the current work stretch is.  Shown even when the daemon
    // is down, because it comes from rest-break, not from stasis.
    if (root.restBreakOnline && root.workMinutes >= 1) return formatMinutes(root.workMinutes)
    return ""
  }

  readonly property string icon: {
    if (!online) return "󰒲"   // md-sleep
    if (locked) return "󰌾"    // md-lock
    return "󰒲"
  }

  readonly property string statusText: {
    if (!online) return "Daemon offline"
    if (locked) return "Resting — " + clockText(remainingSeconds) + " left"
    if (failed) return "Last rest failed"
    if (starting) return "Starting rest…"
    return "Ready"
  }

  readonly property string restBreakTooltip: {
    if (!root.restBreakOnline) return ""
    var lines = ["Working " + formatMinutes(root.workMinutes) +
                 " · fatigue " + formatMinutes(root.fatigueMinutes)]
    if (root.nextRestAt.length > 0) {
      lines.push("Next rest in " + formatMinutes(root.nextRestMinutes) +
                 " (" + hhmm(root.nextRestAt) + ")")
    }
    return lines.join("\n")
  }

  readonly property string tooltip: {
    var base = ""
    if (!online) base = "Stasis: daemon offline\nsystemctl --user start stasis"
    else if (locked) base = "Stasis: resting — " + clockText(remainingSeconds) + " left\nUnlock: press j three times, then your password"
    else if (failed) base = "Stasis: last rest failed — see `stasis status`"
    else if (starting) base = "Stasis: starting rest…"
    else base = "Stasis: ready — click for rest lengths"
    return root.restBreakTooltip.length > 0 ? base + "\n" + root.restBreakTooltip : base
  }

  function refresh() {
    root.nowMs = Date.now()
    var parsed = null
    try {
      parsed = JSON.parse(String(statusFileView.text() || ""))
    } catch (e) {
      parsed = null
    }
    if (!parsed || typeof parsed !== "object" || parsed.running !== true) {
      root.state = ({})
      root.online = false
      root.locked = false
      root.phase = ""
      root.unlockAt = ""
      root.failed = false
      return
    }
    var updated = Date.parse(String(parsed.updatedAt || ""))
    var age = isNaN(updated) ? Infinity : (Date.now() - updated)
    root.online = age >= 0 && age <= 5000
    root.state = parsed
    root.locked = parsed.locked === true
    root.phase = String(parsed.session && parsed.session.phase ? parsed.session.phase : "")
    root.unlockAt = String(parsed.session && parsed.session.unlockAt ? parsed.session.unlockAt : "")
    // A stale failure message outlives its session: only report failure while
    // an unfinished rest session exists, otherwise the icon stays red forever
    // after a single failed attempt.
    root.failed = parsed.ok === false && !root.locked && root.phase === "waiting"
  }

  function startRest(minutes) {
    if (!root.online || !root.bar) return
    root.bar.run(Util.shellQuote(root.binary) + " rest " + Math.round(minutes))
    root.close()
    // No optimistic state: the engine publishes the session within a tick, and
    // guessing here would show a countdown for a request that never landed.
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  FileView {
    id: statusFileView
    path: root.statusFile
    watchChanges: true
    atomicWrites: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: root.refresh()
    onLoadFailed: {
      root.online = false
      root.locked = false
      root.failed = false
    }
  }

  FileView {
    id: restBreakView
    path: root.restBreakCandidates[Math.min(root.restBreakIndex, root.restBreakCandidates.length - 1)]
    watchChanges: true
    atomicWrites: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: root.parseRestBreak(restBreakView.text())
    onLoadFailed: root.restBreakLoadFailed()
  }

  // Countdown ticks locally; the periodic reload is only a safety net for
  // missed inotify events (the engine replaces the file by rename).
  Timer {
    interval: 1000
    running: true
    repeat: true
    onTriggered: root.nowMs = Date.now()
  }

  Timer {
    interval: 5000
    running: true
    repeat: true
    triggeredOnStart: true
    onTriggered: {
      statusFileView.reload()
      // Re-walk the candidate list: rest-break may not have written anything
      // when the widget loaded, and a stale index would never retry path 0.
      if (!root.restBreakOnline && root.restBreakIndex !== 0) {
        root.restBreakIndex = 0
      } else {
        restBreakView.reload()
      }
    }
  }

  IpcHandler {
    target: "stasis.rest"

    function refresh(): void { root.broadcast("refresh") }
    function open(): void { root.open() }
    function close(): void { root.close() }
    function toggle(): void { root.togglePanel() }
    function rest(minutes: int): void { root.startRest(Number(minutes)) }
  }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: root.icon + (root.label.length > 0 ? " " + root.label : "")
    labelVisible: true
    dimmed: !root.online
    active: root.failed
    tooltipText: root.tooltip
    onPressed: function(b) {
      if (b === Qt.LeftButton) root.togglePanel()
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.panelOpen
    centerOnBar: true
    focusTarget: panelBody
    contentWidth: panel.fittedContentWidth(Style.space(320))
    contentHeight: panel.fittedContentHeight(panelBody.implicitHeight + Style.space(8))

    Item {
      id: panelBody
      anchors.fill: parent
      focus: true
      implicitHeight: column.implicitHeight
      Keys.onEscapePressed: root.close()

      Column {
        id: column
        width: parent.width
        spacing: Style.space(10)

        Row {
          width: parent.width
          spacing: Style.space(8)

          Text {
            id: titleLabel
            text: "STASIS"
            color: Commons.Color.popups.text
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.caption
            font.letterSpacing: 1.5
          }

          Item {
            width: Math.max(0, parent.width - titleLabel.implicitWidth - statusLabel.implicitWidth - 2 * parent.spacing)
            height: 1
          }

          Text {
            id: statusLabel
            text: root.statusText
            color: root.failed ? Commons.Color.urgent : Commons.Color.popups.text
            opacity: root.failed ? 1.0 : 0.65
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.caption
          }
        }

        Text {
          width: parent.width
          visible: !root.online
          wrapMode: Text.WordWrap
          text: "Start the daemon first:\nsystemctl --user start stasis"
          color: Commons.Color.popups.text
          opacity: 0.75
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.body
        }

        Text {
          width: parent.width
          visible: root.online
          wrapMode: Text.WordWrap
          text: root.locked
            ? "Resting. The machine unlocks itself when the timer ends."
            : "Pick a rest length. Keyboard and mouse freeze until the timer ends."
          color: Commons.Color.popups.text
          opacity: 0.75
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.body
        }

        Grid {
          width: parent.width
          visible: root.restBreakOnline
          columns: 2
          columnSpacing: Style.space(12)
          rowSpacing: Style.space(3)

          Text {
            width: Style.space(84)
            text: "WORKING"
            color: Commons.Color.popups.text
            opacity: 0.55
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.caption
            font.letterSpacing: 1.2
          }

          Text {
            text: root.formatMinutes(root.workMinutes) +
                  "  ·  fatigue " + root.formatMinutes(root.fatigueMinutes)
            color: Commons.Color.popups.text
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.body
          }

          Text {
            width: Style.space(84)
            text: "NEXT REST"
            color: Commons.Color.popups.text
            opacity: 0.55
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.caption
            font.letterSpacing: 1.2
          }

          Text {
            text: root.nextRestAt.length > 0
              ? "in " + root.formatMinutes(root.nextRestMinutes) + "  ·  " + root.hhmm(root.nextRestAt)
              : "not scheduled"
            color: Commons.Color.popups.text
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.body
          }
        }

        Row {
          width: parent.width
          spacing: Style.space(6)
          visible: root.online && !root.locked

          Repeater {
            model: root.durations

            delegate: Rectangle {
              id: durationButton
              required property int modelData
              readonly property bool hovered: buttonArea.containsMouse
              width: (column.width - Style.space(6) * (root.durations.length - 1)) / root.durations.length
              height: Style.space(34)
              radius: Style.cornerRadius
              // Alpha on the color, not item opacity: opacity would wash out
              // the label along with the background.
              color: Qt.rgba(Commons.Color.popups.text.r,
                             Commons.Color.popups.text.g,
                             Commons.Color.popups.text.b,
                             hovered ? 0.18 : 0.08)

              Text {
                anchors.centerIn: parent
                text: durationButton.modelData + "m"
                color: Commons.Color.popups.text
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.body
              }

              MouseArea {
                id: buttonArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: root.startRest(durationButton.modelData)
              }
            }
          }
        }

        Text {
          width: parent.width
          wrapMode: Text.WordWrap
          text: "Unlock early: press j three times, then your password."
          color: Commons.Color.popups.text
          opacity: 0.55
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.caption
        }
      }
    }
  }
}
