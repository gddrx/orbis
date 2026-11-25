import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts

// The bottom bar: the current track at the left; in the middle the controls
// between the thumbs, over the progress; at the right "add to playlist", the
// track menu, downloads and volume. The left and right parts are as wide, so
// the controls stay in the middle of the window.
Rectangle {
    id: bar

    required property var backend
    required property var shell
    required property var covers
    required property var theme
    required property var trackMenu
    required property var playlistDialog
    required property var downloadsModel

    function time(seconds) {
        const s = Math.max(0, Math.floor(seconds))
        const m = Math.floor(s / 60)
        return m + ":" + String(s % 60).padStart(2, "0")
    }

    // The volume before the mute button turned it off.
    property real unmutedVolume: 60
    // The width of the left and right parts.
    readonly property real side: Math.max(170, Math.min(320, width * 0.24))
    // Between the thumbs and the transport buttons: less in a narrow window.
    readonly property real thumbsGap: width >= 800 ? 12 : 2

    implicitHeight: 92
    color: theme.surface

    RowLayout {
        anchors.left: parent.left
        anchors.leftMargin: 12
        anchors.verticalCenter: parent.verticalCenter
        width: bar.side
        spacing: 10

        Cover {
            Layout.preferredWidth: 56
            Layout.preferredHeight: 56
            covers: bar.covers
            url: bar.backend.nowThumbnail
        }
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Label {
                Layout.fillWidth: true
                text: bar.backend.nowTitle
                color: bar.theme.text
                font.bold: true
                elide: Text.ElideRight
            }
            Label {
                Layout.fillWidth: true
                text: bar.backend.nowArtists
                color: bar.theme.dim
                font.pixelSize: 12
                elide: Text.ElideRight
            }
        }
    }

    ColumnLayout {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.leftMargin: bar.side + 28
        anchors.rightMargin: bar.side + 28
        anchors.verticalCenter: parent.verticalCenter
        spacing: 0

        RowLayout {
            Layout.alignment: Qt.AlignHCenter
            spacing: 6

            IconButton {
                Layout.rightMargin: bar.thumbsGap
                theme: bar.theme
                enabled: bar.backend.nowVideoId.length > 0
                iconName: bar.backend.likeStatus === "dislike" ? "thumb-down-filled" : "thumb-down"
                iconColor: bar.backend.likeStatus === "dislike" ? bar.theme.text : bar.theme.dim
                tip: bar.backend.likeStatus === "dislike" ? bar.theme.tr("Remove dislike") : bar.theme.tr("Dislike")
                onClicked: bar.backend.dislike()
            }
            IconButton {
                theme: bar.theme
                iconName: "previous"
                iconSize: 22
                iconColor: bar.theme.text
                tip: bar.theme.tr("Previous")
                onClicked: bar.backend.previous()
            }
            IconButton {
                implicitWidth: 44
                implicitHeight: 44
                theme: bar.theme
                filled: true
                fillColor: bar.theme.text
                iconName: bar.backend.playing ? "pause" : "play"
                iconSize: 24
                iconColor: bar.theme.background
                tip: bar.backend.playing ? bar.theme.tr("Pause") : bar.theme.tr("Play")
                onClicked: bar.backend.togglePause()

                // A track is being started: an arc turns around the button.
                Spinner {
                    anchors.centerIn: parent
                    width: 52
                    height: 52
                    tint: bar.theme.accent
                    visible: bar.backend.starting.length > 0
                }
            }
            IconButton {
                theme: bar.theme
                iconName: "next"
                iconSize: 22
                iconColor: bar.theme.text
                tip: bar.theme.tr("Next")
                onClicked: bar.backend.next()
            }
            IconButton {
                Layout.leftMargin: bar.thumbsGap
                theme: bar.theme
                enabled: bar.backend.nowVideoId.length > 0
                iconName: bar.backend.likeStatus === "like" ? "thumb-up-filled" : "thumb-up"
                iconColor: bar.backend.likeStatus === "like" ? bar.theme.accent : bar.theme.dim
                hoverIconColor: bar.backend.likeStatus === "like" ? bar.theme.accent : bar.theme.text
                tip: bar.backend.likeStatus === "like" ? bar.theme.tr("Remove from liked") : bar.theme.tr("Like")
                onClicked: bar.backend.like()
            }
        }

        RowLayout {
            Layout.fillWidth: true
            spacing: 10

            Label {
                Layout.preferredWidth: 40
                horizontalAlignment: Text.AlignRight
                text: bar.time(progress.pressed ? progress.value : bar.backend.position)
                color: bar.theme.faint
                font.pixelSize: 12
                font.features: { "tnum": 1 }
            }
            Seek {
                id: progress

                Layout.fillWidth: true
                theme: bar.theme
                from: 0
                to: Math.max(1, bar.backend.duration)
                value: bar.backend.position
                onMoved: bar.backend.seek(value)
            }
            Label {
                Layout.preferredWidth: 40
                text: bar.time(bar.backend.duration)
                color: bar.theme.faint
                font.pixelSize: 12
                font.features: { "tnum": 1 }
            }
        }
    }

    RowLayout {
        anchors.right: parent.right
        anchors.rightMargin: 16
        anchors.verticalCenter: parent.verticalCenter
        width: bar.side
        spacing: 2

        // Pushes the buttons to the right when the part is wider than they.
        Item {
            Layout.fillWidth: true
        }
        IconButton {
            theme: bar.theme
            iconName: "playlist-add"
            tip: bar.theme.tr("Add to playlist")
            enabled: bar.backend.nowVideoId.length > 0
            onClicked: {
                bar.backend.menuForCurrent()
                bar.playlistDialog.open()
            }
        }
        IconButton {
            theme: bar.theme
            iconName: "more"
            tip: bar.theme.tr("Track menu")
            enabled: bar.backend.nowVideoId.length > 0
            onClicked: {
                bar.backend.menuForCurrent()
                bar.trackMenu.popup()
            }
        }
        // Downloads of this run: shown once there are any. Its visibility
        // follows the model and its badge the backend, never both in one
        // binding: QML that reads the backend while it resets a model
        // re-enters it, and Qt Bridge for Rust crashes the window then.
        IconButton {
            id: downloadsButton

            visible: bar.downloadsModel.count > 0
            theme: bar.theme
            iconName: "download"
            iconColor: bar.backend.downloadsActive > 0 ? bar.theme.accent : bar.theme.dim
            tip: bar.theme.tr("Downloads")
            onClicked: downloadsPopup.opened ? downloadsPopup.close() : downloadsPopup.open()

            Rectangle {
                visible: bar.backend.downloadsActive > 0
                anchors.right: parent.right
                anchors.top: parent.top
                width: Math.max(16, badge.implicitWidth + 8)
                height: 16
                radius: 8
                color: bar.theme.accent

                Label {
                    id: badge

                    anchors.centerIn: parent
                    text: bar.backend.downloadsActive
                    color: bar.theme.background
                    font.pixelSize: 10
                    font.bold: true
                }
            }

            // The smoke test opens and closes the list for the bench's pictures.
            Connections {
                target: bar.backend
                function onDownloadsListRequested(open) {
                    open ? downloadsPopup.open() : downloadsPopup.close()
                    // Later: the backend does not let QML in while it asks.
                    Qt.callLater(bar.backend.smokeWindowShown, open ? "downloads" : "",
                                 downloadsPopup.shown)
                }
            }
            // Made on its first opening.
            OnDemand {
                id: downloadsPopup

                sourceComponent: DownloadsPopup {
                    x: downloadsButton.width - width
                    y: -height - 8
                    backend: bar.backend
                    shell: bar.shell
                    model: bar.downloadsModel
                    theme: bar.theme
                }
            }
        }
        IconButton {
            theme: bar.theme
            iconName: bar.backend.volume > 0 ? "volume" : "volume-off"
            tip: bar.backend.volume > 0 ? bar.theme.tr("Mute") : bar.theme.tr("Unmute")
            onClicked: {
                if (bar.backend.volume > 0) {
                    bar.unmutedVolume = bar.backend.volume
                    bar.backend.setVolume(0)
                } else {
                    bar.backend.setVolume(bar.unmutedVolume > 0 ? bar.unmutedVolume : 60)
                }
            }
        }
        Seek {
            // Too narrow a window keeps the mute button only.
            visible: bar.width >= 760
            Layout.fillWidth: true
            Layout.minimumWidth: 60
            Layout.maximumWidth: 110
            theme: bar.theme
            from: 0
            to: 100
            value: bar.backend.volume
            onMoved: bar.backend.setVolume(value)
        }
    }
}
