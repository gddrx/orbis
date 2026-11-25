import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts

// "Add to playlist" for the track the backend's menu points at: the
// account's playlists, or a new playlist with the track.
Popup {
    id: dialog

    required property var backend
    required property var optionsModel
    required property var theme

    function create() {
        const title = name.text.trim()
        if (title.length === 0)
            return
        dialog.backend.menuCreatePlaylist(title, privacy.currentValue)
        dialog.close()
    }

    anchors.centerIn: Overlay.overlay
    width: 380
    height: Math.min(540, Overlay.overlay ? Overlay.overlay.height - 60 : 540)
    modal: true
    focus: true
    padding: 16
    background: Rectangle {
        color: dialog.theme.raised
        radius: 10
    }
    onOpened: {
        name.text = ""
        dialog.backend.menuLoadPlaylists()
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 8

        Label {
            text: dialog.theme.tr("Add to playlist")
            color: dialog.theme.text
            font.pixelSize: 18
            font.bold: true
        }
        Label {
            Layout.fillWidth: true
            text: dialog.backend.menuTitle
            color: dialog.theme.dim
            elide: Text.ElideRight
        }

        ListView {
            id: list

            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: dialog.optionsModel

            delegate: ItemDelegate {
                id: option

                required property string title
                required property string subtitle
                required property string target_id

                width: ListView.view.width
                contentItem: ColumnLayout {
                    spacing: 1
                    Label {
                        Layout.fillWidth: true
                        text: option.title
                        color: dialog.theme.text
                        elide: Text.ElideRight
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: text.length > 0
                        text: option.subtitle
                        color: dialog.theme.dim
                        font.pixelSize: 12
                        elide: Text.ElideRight
                    }
                }
                onClicked: {
                    dialog.backend.menuAddTo(option.target_id, option.title)
                    dialog.close()
                }
            }

            BusyIndicator {
                anchors.centerIn: parent
                running: dialog.opened && list.count === 0
                visible: running
            }
            ScrollBar.vertical: ScrollBar {}
        }

        Label {
            Layout.topMargin: 6
            text: dialog.theme.tr("New playlist with this track")
            color: dialog.theme.text
            font.bold: true
        }
        Field {
            id: name
            Layout.fillWidth: true
            theme: dialog.theme
            placeholderText: dialog.theme.tr("Name")
            onAccepted: dialog.create()
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: 8

            Choice {
                id: privacy
                Layout.fillWidth: true
                theme: dialog.theme
                textRole: "text"
                valueRole: "value"
                model: [
                    { text: dialog.theme.tr("Private"), value: "private" },
                    { text: dialog.theme.tr("Unlisted"), value: "unlisted" },
                    { text: dialog.theme.tr("Public"), value: "public" }
                ]
            }
            TextButton {
                theme: dialog.theme
                accent: true
                text: dialog.theme.tr("Create")
                enabled: name.text.trim().length > 0
                onClicked: dialog.create()
            }
        }
    }
}
