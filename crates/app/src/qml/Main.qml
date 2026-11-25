import QtQuick

// The root: lives as long as the application. The window comes and goes with
// `shell`: hidden in the tray it is destroyed, unless only hiding is wanted,
// and only the core plays on.
QtObject {
    id: root

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

    // A quit was asked for: the window closes instead of going to the tray.
    // Qt asks every window to close before it quits, and one that refuses
    // cancels the quit.
    property bool quitting: false

    // The tray and MPRIS show the current track and playback.
    function showTrack() {
        const b = root.backend
        root.shell.showTrack(b.nowVideoId, b.nowTitle, b.nowArtists, b.nowThumbnail,
                             b.duration, b.playing, b.likeStatus, b.volume)
    }

    Component.onCompleted: {
        showTrack()
        root.shell.setSignedIn(root.backend.signedIn)
    }

    // An Instantiator, not a Loader: a window made by a Loader follows the
    // Loader item's visibility, and an item outside any scene is never seen.
    property Instantiator window: Instantiator {
        active: root.shell.windowVisible || !root.shell.unloadHidden
        delegate: MainWindow {
            backend: root.backend
            covers: root.covers
            glass: root.glass
            shell: root.shell
            settings: root.settings
            tabsModel: root.tabsModel
            tracksModel: root.tracksModel
            cardsModel: root.cardsModel
            shelvesModel: root.shelvesModel
            optionsModel: root.optionsModel
            downloadsModel: root.downloadsModel
            quitting: root.quitting
        }
        // The window's objects are deleted a moment later: then the JS heap
        // is collected and freed memory goes back to the system.
        onObjectRemoved: root.unloaded.restart()
    }

    property Timer unloaded: Timer {
        interval: 1000
        onTriggered: {
            gc()
            root.shell.trimMemory()
        }
    }

    // The tray's and MPRIS's "quit" and the smoke test quit through the
    // backend; seeks are told to MPRIS's clients.
    property Connections events: Connections {
        target: root.backend
        function onNowChanged() { root.showTrack() }
        function onPlayingChanged() { root.showTrack() }
        function onLikeChanged() { root.showTrack() }
        function onVolumeChanged() { root.showTrack() }
        // The tray tells when a sign-in is needed: the window may be away.
        function onSignedInChanged() { root.shell.setSignedIn(root.backend.signedIn) }
        function onSeeked(position) { root.shell.mediaSeeked(position) }
        function onQuitRequested() {
            root.quitting = true
            Qt.quit()
        }
    }
}
