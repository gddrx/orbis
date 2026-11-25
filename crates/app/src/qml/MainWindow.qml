import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts

// The window. Main.qml creates it, and destroys it when it is hidden in the
// tray and the interface is unloaded there.
ApplicationWindow {
    id: window

    required property var backend
    required property var covers
    required property var glass
    required property var shell
    required property var settings
    required property var tabsModel
    required property var tracksModel
    required property var cardsModel
    required property var shelvesModel
    required property var optionsModel
    required property var downloadsModel
    // The application quits: closing must not hide the window in the tray.
    property bool quitting: false

    readonly property Theme theme: Theme {
        translucent: window.glass.blur
        coverAccent: window.glass.accent
        groundOpacity: window.glass.opacity
        strings: window.settings.strings
    }
    // The current track's cover, blurred, lies under everything.
    readonly property string coverUrl: backend.nowThumbnail

    width: 1100
    height: 720
    minimumWidth: 640
    minimumHeight: 420
    visible: shell.windowVisible
    title: backend.nowTitle.length > 0 ? backend.nowTitle + " — orbis" : "orbis"
    // The window has an alpha channel; its content paints the ground, which
    // is translucent only when the compositor blurs behind it.
    color: "transparent"

    // A dark palette for every control of the Basic style.
    palette.window: theme.background
    palette.windowText: theme.text
    palette.base: theme.surface
    palette.alternateBase: theme.raised
    palette.text: theme.text
    // Tool buttons always draw their background (at half opacity): glass.
    palette.button: theme.hover
    palette.buttonText: theme.text
    palette.highlight: theme.accent
    palette.highlightedText: theme.background
    palette.light: theme.lifted
    palette.midlight: theme.raised
    palette.mid: theme.faint
    palette.dark: theme.surface
    palette.shadow: "#000000"
    palette.placeholderText: theme.faint
    palette.toolTipBase: theme.raised
    palette.toolTipText: theme.text

    Connections {
        target: window.backend
        // Popups are not in the picture: they live in Qt's overlay, an item
        // made in C++ that cannot be grabbed.
        function onScreenshotRequested(path) {
            content.grabToImage(result => result.saveToFile(path))
        }
        function onSignedInChanged() { window.askSignIn() }
        // Later: opening, the dialogs call the backend, which does not let
        // QML in while it asks.
        function onWindowRequested(name) { Qt.callLater(window.smokeWindow, name) }
    }

    // The smoke test opens the windows made on first use, one at a time,
    // and hears whether each is shown; an empty name closes them. Their
    // pictures are the bench's: the window's own picture leaves popups out.
    function smokeWindow(name) {
        const windows = { menu: trackMenu, playlist: playlistDialog, login: loginDialog }
        for (const key in windows)
            windows[key].close()
        const wanted = windows[name]
        if (name === "menu")
            wanted.popup()
        else if (wanted)
            wanted.open()
        window.backend.smokeWindowShown(name, wanted ? wanted.shown : false)
    }

    // The tray's menu asks for the settings; the window may only now be
    // built for them.
    function openWantedSettings() {
        if (window.shell.settingsWanted) {
            window.openSettings()
            window.shell.settingsOpened()
        }
    }
    // The settings dialog is made when opened and dropped when closed: it
    // and the system's folder dialog cost nothing until wanted.
    function openSettings() {
        if (settingsLoader.item)
            settingsLoader.item.open()
        else
            settingsLoader.active = true
    }
    Connections {
        target: window.shell
        function onSettingsChanged() { window.openWantedSettings() }
        // No file manager selected the downloaded file: its folder opens.
        function onFolderWanted(folder) { Qt.openUrlExternally(folder) }
    }

    // Without a sign-in nothing works: the dialog asks for one when the
    // window opens and whenever the session stops being accepted.
    function askSignIn() {
        if (window.backend.signedIn)
            loginDialog.close()
        else
            loginDialog.open()
    }

    // Closing the window hides it in the tray when a tray shows the icon;
    // without one it closes, and the application quits. The hiding waits for
    // the close event to end: it may destroy this very window. A quit (the
    // tray's, MPRIS's) closes it too: refused, the close would cancel it.
    onClosing: close => {
        if (window.shell.trayShown && !window.quitting) {
            close.accepted = false
            Qt.callLater(window.shell.hideWindow)
        }
    }

    // The blur is asked for once the window exists; whether the compositor
    // does it is known a round trip later, and is read again on activation
    // in case the effect was turned off or on.
    Component.onCompleted: {
        window.glass.enableBlur()
        Qt.callLater(window.askSignIn)
        Qt.callLater(window.openWantedSettings)
    }
    // Shown again from the tray (its "Sign in…" among others): the dialog
    // comes back if the session is still not accepted.
    onVisibleChanged: if (visible) Qt.callLater(window.askSignIn)
    onActiveChanged: {
        window.glass.checkBlur()
        window.shell.windowActivated(active)
    }
    onCoverUrlChanged: window.glass.showCover(coverUrl)
    Timer {
        interval: 300
        running: true
        onTriggered: window.glass.checkBlur()
    }

    // The first frame ends the start (timed for the smoke test); then
    // nothing listens.
    property bool framed: false
    Connections {
        target: window
        enabled: !window.framed
        function onFrameSwapped() {
            window.framed = true
            window.shell.frameShown()
        }
    }

    // The smoke test counts the frames drawn; nothing listens otherwise.
    Connections {
        target: window
        enabled: window.glass.countingFrames
        function onFrameSwapped() { window.glass.countFrame() }
    }

    Shortcut {
        sequences: ["Ctrl+F", "Ctrl+L"]
        onActivated: tabStrip.focusSearch()
    }

    // The position is read once a second, only while the window is seen and
    // something plays: nothing redraws otherwise.
    Timer {
        interval: 1000
        repeat: true
        running: window.visible && window.visibility !== Window.Minimized && window.backend.playing
        onTriggered: window.backend.refreshPosition()
    }

    // The background is part of the content, so pictures of it match the window.
    Rectangle {
        id: content

        anchors.fill: parent
        color: window.theme.base

        Backdrop {
            anchors.fill: parent
            source: window.glass.backdrop
            opacity: window.theme.backdropOpacity
        }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        TabStrip {
            id: tabStrip
            Layout.fillWidth: true
            model: window.tabsModel
            backend: window.backend
            theme: window.theme
            onSettingsClicked: window.openSettings()
        }

        // The tabs opened since, at the left, when there are any.
        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 0

            TabSidebar {
                Layout.fillHeight: true
                Layout.preferredWidth: 230
                visible: window.backend.sideTabs
                model: window.tabsModel
                backend: window.backend
                theme: window.theme
            }

            // Shelves for the home page, artists and search, a grid for the
            // library, tracks for the rest.
            StackLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                currentIndex: {
                    const kind = window.backend.shownKind
                    if (kind === "home" || kind === "artist" || kind === "search")
                        return 1
                    if (kind === "library")
                        return 2
                    return 0
                }

                TrackList {
                    model: window.tracksModel
                    backend: window.backend
                    covers: window.covers
                    theme: window.theme
                    trackMenu: trackMenu
                }
                ShelfList {
                    model: window.shelvesModel
                    backend: window.backend
                    covers: window.covers
                    theme: window.theme
                    trackMenu: trackMenu
                }
                CardGrid {
                    model: window.cardsModel
                    backend: window.backend
                    covers: window.covers
                    theme: window.theme
                }
            }
        }

        Rectangle {
            Layout.fillWidth: true
            implicitHeight: statusRow.implicitHeight
            visible: statusText.text.length > 0
            color: window.theme.surface

            RowLayout {
                id: statusRow

                anchors.left: parent.left
                anchors.right: parent.right
                anchors.rightMargin: 8
                spacing: 8

                Label {
                    id: statusText

                    Layout.fillWidth: true
                    text: window.backend.status.length > 0 || window.backend.signedIn
                        ? window.backend.status : window.theme.tr("Not signed in to YouTube Music.")
                    color: window.backend.signedIn ? window.theme.dim : window.theme.danger
                    wrapMode: Text.Wrap
                    padding: 8
                }
                TextButton {
                    visible: !window.backend.signedIn
                    theme: window.theme
                    accent: true
                    text: window.theme.tr("Sign in…")
                    onClicked: loginDialog.open()
                }
            }
        }

        PlayerBar {
            Layout.fillWidth: true
            backend: window.backend
            shell: window.shell
            covers: window.covers
            theme: window.theme
            trackMenu: trackMenu
            playlistDialog: playlistDialog
            downloadsModel: window.downloadsModel
        }
    }
    }

    // Opened now and then, so made on their first use; the sign-in dialog
    // at the start only when a sign-in is needed.
    OnDemand {
        id: trackMenu
        sourceComponent: TrackMenu {
            backend: window.backend
            theme: window.theme
            playlistDialog: playlistDialog
        }
    }
    OnDemand {
        id: playlistDialog
        sourceComponent: PlaylistDialog {
            backend: window.backend
            optionsModel: window.optionsModel
            theme: window.theme
        }
    }
    OnDemand {
        id: loginDialog
        sourceComponent: LoginDialog {
            backend: window.backend
            theme: window.theme
        }
    }
    // Made only while «Download all» asks about a long list.
    Loader {
        active: window.backend.downloadAsk > 0
        sourceComponent: DownloadQuestion {
            backend: window.backend
            theme: window.theme
        }
        onLoaded: item.open()
    }

    Loader {
        id: settingsLoader

        active: false
        sourceComponent: SettingsDialog {
            id: settingsDialog

            settings: window.settings
            backend: window.backend
            theme: window.theme
            onSignInRequested: {
                settingsDialog.close()
                loginDialog.open()
            }
            onClosed: Qt.callLater(() => { settingsLoader.active = false })
        }
        onLoaded: item.open()
    }

}
