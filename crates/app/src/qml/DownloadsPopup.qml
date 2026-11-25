import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts

// The downloads of this run: each track with its progress and stage. What
// waits or runs can be cancelled, a saved track shown in its folder.
Popup {
    id: popup

    required property var backend
    required property var shell
    required property var model
    required property var theme

    function stageText(stage, share, message) {
        switch (stage) {
        case "queued": return popup.theme.tr("Queued")
        case "active": return share > 0 ? popup.theme.tr("Downloading — %1 %").arg(Math.round(share * 100))
                                        : popup.theme.tr("Looking for the stream…")
        case "done": return popup.theme.tr("Saved")
        case "skipped": return popup.theme.tr("Already downloaded")
        case "failed": return popup.theme.tr("Error: %1").arg(message)
        case "cancelled": return popup.theme.tr("Cancelled")
        }
        return ""
    }

    width: 440
    height: Math.min(480, list.contentHeight + 76)
    padding: 12
    focus: true
    background: Rectangle {
        color: popup.theme.raised
        radius: 10
    }

    contentItem: ColumnLayout {
        spacing: 8

        RowLayout {
            Layout.fillWidth: true
            spacing: 4

            Label {
                Layout.fillWidth: true
                text: popup.theme.tr("Downloads")
                color: popup.theme.text
                font.pixelSize: 16
                font.bold: true
            }
            TextButton {
                visible: popup.backend.downloadsActive > 0
                theme: popup.theme
                text: popup.theme.tr("Cancel all")
                flat: true
                onClicked: popup.backend.cancelDownloads()
            }
            TextButton {
                theme: popup.theme
                text: popup.theme.tr("Clear")
                flat: true
                onClicked: popup.backend.clearDownloads()
            }
        }

        ListView {
            id: list

            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            reuseItems: true
            spacing: 4
            boundsBehavior: Flickable.StopAtBounds
            model: popup.model

            delegate: Item {
                id: row

                required property int download_id
                required property string title
                required property string artists
                required property string stage
                required property real share
                required property string file
                required property string folder
                required property string message

                readonly property bool open: stage === "queued" || stage === "active"

                width: ListView.view.width
                height: 58

                RowLayout {
                    anchors.fill: parent
                    spacing: 8

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2

                        Label {
                            Layout.fillWidth: true
                            text: row.artists.length > 0 ? row.title + " — " + row.artists : row.title
                            color: popup.theme.text
                            elide: Text.ElideRight
                        }
                        ProgressBar {
                            Layout.fillWidth: true
                            visible: row.stage === "active"
                            value: row.share
                        }
                        Label {
                            Layout.fillWidth: true
                            text: popup.stageText(row.stage, row.share, row.message)
                            color: row.stage === "failed" ? popup.theme.danger : popup.theme.dim
                            font.pixelSize: 12
                            elide: Text.ElideRight
                        }
                    }
                    IconButton {
                        visible: row.open
                        theme: popup.theme
                        iconName: "close"
                        tip: popup.theme.tr("Cancel")
                        onClicked: popup.backend.cancelDownload(row.download_id)
                    }
                    IconButton {
                        visible: row.folder.length > 0
                        theme: popup.theme
                        iconName: "folder"
                        tip: popup.theme.tr("Show in folder")
                        onClicked: popup.shell.showInFolder(row.file, row.folder)
                    }
                }
            }
        }
    }
}
