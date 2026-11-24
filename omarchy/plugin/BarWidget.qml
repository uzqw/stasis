import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
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

  // rest-break reports work inside a rolling three hour window, so its number
  // keeps the work done before a rest.  The daemon also records when the last
  // rest ended; counting from there makes the clock visibly restart after a
  // rest, which is what "how long have I been working" means to a user.
  readonly property double sinceRestMinutes: {
    var iso = String(root.state && root.state.lastRestEnd ? root.state.lastRestEnd : "")
    var t = Date.parse(iso)
    if (isNaN(t)) return 0
    return Math.max(0, (root.nowMs - t) / 60000)
  }
  readonly property double restBreakWorkMinutes: {
    var v = Number(root.restBreak ? root.restBreak.workMinutes : NaN)
    return isFinite(v) && v > 0 ? v : 0
  }
  readonly property double workMinutes: root.sinceRestMinutes > 0 ? root.sinceRestMinutes : root.restBreakWorkMinutes
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
  /// Minutes until the predicted rest; <= 0 means the model says "now".
  readonly property double minutesUntilNextRest: {
    var t = Date.parse(root.nextRestAt)
    if (isNaN(t)) return -1
    return (t - root.nowMs) / 60000
  }

  property var state: ({})
  property bool online: false
  property bool locked: false
  property string sessionId: ""
  property string phase: ""
  property string lockAt: ""
  property string unlockAt: ""
  property bool failed: false
  property double nowMs: Date.now()

  // Multi-screen pre-lock toast: Omarchy's D-Bus notification daemon only
  // renders on the focused display, whereas a BarWidget instance lives on
  // every screen's bar.  Each instance maps a non-focus-stealing layer-shell
  // toast on its own screen so the 1-minute warning is visible everywhere.
  readonly property var barWindow: root.QsWindow.window
  readonly property var barScreen: barWindow ? barWindow.screen : null
  readonly property real barThicknessOnScreen: {
    if (!root.bar || root.bar.barHidden) return 0
    if (typeof root.bar.barSizeFor === "function" && root.barScreen) {
      return Number(root.bar.barSizeFor(root.barScreen.name)) || root.barSize
    }
    return root.barSize
  }
  property bool toastOpen: false
  property string toastSummary: ""
  property string toastBody: ""
  property string lastNotifiedSessionId: ""

  onLockedChanged: {
    if (root.locked) root.hideToast()
  }

  function allInstances() {
    return root.bar && typeof root.bar.moduleWidgets === "function"
      ? root.bar.moduleWidgets(root.moduleName) : [root]
  }

  function defaultToastBody() {
    var start = Date.parse(root.lockAt)
    var end = Date.parse(root.unlockAt)
    var mins = (!isNaN(start) && !isNaN(end) && end > start)
      ? Math.max(1, Math.round((end - start) / 60000)) : 10
    if (root.lockInSeconds >= 45) {
      return "1 分钟后将锁定键鼠休息 " + mins + " 分钟，请提前保存当前工作。"
    }
    var secs = Math.max(1, root.lockInSeconds)
    return secs + " 秒后将锁定键鼠休息 " + mins + " 分钟，请提前保存当前工作。"
  }

  function triggerPreLockToast(summary, body) {
    if (root.sessionId !== "") {
      root.lastNotifiedSessionId = root.sessionId
    }
    root.toastSummary = String(summary || "即将锁屏休息")
    root.toastBody = String(body || root.defaultToastBody())
    root.toastOpen = true
    toastHideTimer.restart()
  }

  function hideToast() {
    root.toastOpen = false
    toastHideTimer.stop()
  }

  function dismissToastEverywhere() {
    root.broadcast("hideToast")
  }

  // Panel lifecycle.  `opened` / `open()` / `close()` are the contract the bar
  // uses to route popout ownership and hotkey summoning.
  property bool panelOpen: false
  readonly property bool opened: panelOpen
  function open() { panelOpen = true }
  function close() { panelOpen = false }
  function togglePanel() { panelOpen ? close() : open() }

  readonly property int lockInSeconds: secondsUntil(lockAt)
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
    if (root.starting) return lockInSeconds > 0 ? "in " + clockText(lockInSeconds) : "…"
    // Idle: when the next rest starts.  Fatigue is the model's input, not
    // something the user can act on; "in 42m" answers the actual question.
    if (root.restBreakOnline && root.nextRestAt.length > 0) {
      var minutes = root.minutesUntilNextRest
      return minutes <= 0 ? "now" : "in " + formatMinutes(minutes)
    }
    // No prediction (rest-break offline): fall back to time since the last
    // rest, the other half of "when do I rest".
    if (root.sinceRestMinutes >= 1) return formatMinutes(root.sinceRestMinutes) + " ago"
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
    if (starting) return lockInSeconds > 0 ? "Starting in " + clockText(lockInSeconds) : "Starting rest…"
    return "Ready"
  }

  readonly property string restBreakTooltip: {
    if (!root.restBreakOnline) return ""
    var lines = []
    if (root.nextRestAt.length > 0) {
      lines.push("Next rest in " + formatMinutes(Math.max(0, root.minutesUntilNextRest)) +
                 " (" + hhmm(root.nextRestAt) + ")" +
                 (root.nextRestMinutes > 0 ? " · " + root.nextRestMinutes + "m" : ""))
    }
    lines.push("Fatigue " + formatMinutes(root.fatigueMinutes))
    if (root.workMinutes >= 1) lines.push("Working " + formatMinutes(root.workMinutes))
    return lines.join("\n")
  }

  readonly property string tooltip: {
    var base = ""
    if (!online) base = "Stasis: daemon offline\nsystemctl --user start stasis"
    else if (locked) base = "Stasis: resting — " + clockText(remainingSeconds) + " left\nUnlock: press j three times, then your password"
    else if (failed) base = "Stasis: last rest failed — see `stasis status`"
    else if (starting) base = lockInSeconds > 0 ? "Stasis: locking in " + clockText(lockInSeconds) : "Stasis: starting rest…"
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
      root.sessionId = ""
      root.phase = ""
      root.lockAt = ""
      root.unlockAt = ""
      root.failed = false
      root.hideToast()
      return
    }
    var updated = Date.parse(String(parsed.updatedAt || ""))
    var age = isNaN(updated) ? Infinity : (Date.now() - updated)
    root.online = age >= 0 && age <= 5000
    root.state = parsed
    root.locked = parsed.locked === true
    root.sessionId = String(parsed.session && parsed.session.sessionId ? parsed.session.sessionId : "")
    root.phase = String(parsed.session && parsed.session.phase ? parsed.session.phase : "")
    root.lockAt = String(parsed.session && parsed.session.lockAt ? parsed.session.lockAt : "")
    root.unlockAt = String(parsed.session && parsed.session.unlockAt ? parsed.session.unlockAt : "")
    // A stale failure message outlives its session: only report failure while
    // an unfinished rest session exists, otherwise the icon stays red forever
    // after a single failed attempt.
    root.failed = parsed.ok === false && !root.locked && root.phase === "waiting"

    // Belt-and-suspenders: if status.json enters the 1-minute waiting window
    // before (or without) the IPC call, pop the toast on this screen once per
    // sessionId so no monitor ever misses the heads-up.
    if (root.online && !root.locked && root.phase === "waiting"
        && root.sessionId !== "" && root.sessionId !== root.lastNotifiedSessionId
        && root.lockInSeconds > 0 && root.lockInSeconds <= 62) {
      root.triggerPreLockToast("即将锁屏休息", root.defaultToastBody())
    }
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

  Timer {
    id: toastHideTimer
    interval: 15000
    repeat: false
    onTriggered: root.toastOpen = false
  }

  IpcHandler {
    target: "stasis.rest"

    function refresh(): void { root.broadcast("refresh") }
    function open(): void { root.open() }
    function close(): void { root.close() }
    function toggle(): void { root.togglePanel() }
    function rest(minutes: int): void { root.startRest(Number(minutes)) }
    function dismissToast(): string {
      root.dismissToastEverywhere()
      return "ok"
    }
    function notify(summary: string, body: string): string {
      var items = root.allInstances()
      var shown = 0
      for (var i = 0; i < items.length; i++) {
        if (items[i] && typeof items[i].triggerPreLockToast === "function") {
          items[i].triggerPreLockToast(summary, body)
          shown++
        }
      }
      return shown > 0 ? "ok:" + shown : "none"
    }
    function probe(): string {
      var items = root.allInstances()
      var screens = []
      for (var i = 0; i < items.length; i++) {
        var it = items[i]
        if (!it) continue
        screens.push({
          screen: it.barScreen ? String(it.barScreen.name) : "",
          toastOpen: it.toastOpen === true,
          summary: String(it.toastSummary || "")
        })
      }
      return JSON.stringify({ count: screens.length, screens: screens })
    }
  }

  PanelWindow {
    id: preLockToastWindow
    visible: (root.toastOpen || toastCard.opacity > 0) && !!root.barScreen
    screen: root.barScreen
    color: "transparent"

    WlrLayershell.namespace: "stasis-prelock-toast"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.None
    exclusionMode: ExclusionMode.Ignore

    anchors {
      top: true
      right: true
    }
    margins.top: ((root.bar && root.bar.position === "top") ? root.barThicknessOnScreen : 0) + Style.gapsOut
    margins.right: ((root.bar && root.bar.position === "right") ? root.barThicknessOnScreen : 0) + Style.gapsOut
    implicitWidth: Style.space(380)
    implicitHeight: toastCard.height + Style.space(4)

    mask: Region { item: root.toastOpen ? toastCard : null }

    BorderSurface {
      id: toastCard
      width: Style.space(380)
      height: toastCard.contentTopInset + toastColumn.implicitHeight + toastCard.contentBottomInset
      color: Commons.Color.notifications.background
      borderSpec: Border.surfaceSpec(
        "notifications",
        "border",
        Commons.Color.notifications.border,
        Math.max(1, Style.space(2))
      )
      padding: Style.space(12)
      radius: Style.cornerRadius

      property real shown: root.toastOpen ? 1 : 0
      Behavior on shown {
        id: toastAnim
        NumberAnimation {
          duration: Style.duration(toastAnim.targetValue > 0 ? 180 : 120)
          easing.type: Easing.Bezier
          easing.bezierCurve: Style.easeOut
        }
      }
      opacity: shown
      scale: 0.97 + 0.03 * shown
      transformOrigin: Item.TopRight

      Column {
        id: toastColumn
        x: toastCard.contentLeftInset
        y: toastCard.contentTopInset
        width: toastCard.width - toastCard.contentLeftInset - toastCard.contentRightInset
        spacing: Style.space(6)

        Rectangle {
          width: parent.width
          height: Math.max(2, Style.space(3))
          radius: height / 2
          color: Commons.Color.notifications.countdown
        }

        Row {
          width: parent.width
          spacing: Style.space(8)

          Text {
            id: toastGlyph
            textFormat: Text.PlainText
            text: "󰒲"
            color: Commons.Color.notifications.countdown
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.body
          }

          Text {
            id: toastTitle
            textFormat: Text.PlainText
            width: Math.max(
              0,
              parent.width - toastGlyph.implicitWidth - toastCountdown.implicitWidth
                - (toastCountdown.visible ? 2 : 1) * parent.spacing
            )
            elide: Text.ElideRight
            text: root.toastSummary
            color: Commons.Color.notifications.text
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.body
            font.bold: true
          }

          Text {
            id: toastCountdown
            textFormat: Text.PlainText
            visible: root.starting && root.lockInSeconds > 0
            text: root.clockText(root.lockInSeconds)
            color: Commons.Color.notifications.countdown
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.caption
            font.bold: true
          }
        }

        Text {
          width: parent.width
          textFormat: Text.PlainText
          wrapMode: Text.WordWrap
          visible: root.toastBody.length > 0
          text: root.toastBody
          color: Qt.darker(Commons.Color.notifications.text, 1.12)
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.caption
        }
      }

      MouseArea {
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onClicked: root.dismissToastEverywhere()
      }
    }
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
            text: "FATIGUE"
            color: Commons.Color.popups.text
            opacity: 0.55
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.caption
            font.letterSpacing: 1.2
          }

          Text {
            text: root.formatMinutes(root.fatigueMinutes)
            color: Commons.Color.popups.text
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.body
          }

          Text {
            width: Style.space(84)
            text: "LAST REST"
            color: Commons.Color.popups.text
            opacity: 0.55
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.caption
            font.letterSpacing: 1.2
          }

          Text {
            text: root.sinceRestMinutes >= 1
              ? root.formatMinutes(root.sinceRestMinutes) + " ago"
              : "—"
            color: Commons.Color.popups.text
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.body
          }

          Text {
            width: Style.space(84)
            text: root.locked ? "RESTING" : "NEXT REST"
            color: Commons.Color.popups.text
            opacity: 0.55
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.caption
            font.letterSpacing: 1.2
          }

          Text {
            text: root.locked
              ? "ends " + root.hhmm(root.unlockAt)
              : root.nextRestAt.length > 0
                ? "in " + root.formatMinutes(Math.max(0, root.minutesUntilNextRest)) +
                  "  ·  " + root.hhmm(root.nextRestAt) +
                  (root.nextRestMinutes > 0 ? "  ·  " + root.nextRestMinutes + "m" : "")
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
