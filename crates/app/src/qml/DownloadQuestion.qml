import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts

// «Download all» on a long list asks first: how many tracks the list has.
// The backend keeps the list until the answer; closing the dialog is a "no".
Popup {
    id: dialog

    required property var backend
    required property var theme

    anchors.centerIn: Overlay.overlay
    width: 420
    modal: true
    focus: true
    padding: 20
    closePolicy: Popup.CloseOnEscape
    background: Rectangle {
        color: dialog.theme.raised
        radius: 10
    }
    // Later: the dialog also closes when the backend drops the question,
    // and the backend does not let QML in while it tells so.
    onClosed: Qt.callLater(dialog.backend.answerDownload, false)

    contentItem: ColumnLayout {
        spacing: 8

        Label {
            text: dialog.theme.tr("Download the whole list?")
            color: dialog.theme.text
            font.pixelSize: 18
            font.bold: true
        }
        Label {
            Layout.fillWidth: true
            // The title last: a "%2" in it stays as it is.
            text: dialog.theme.tr("Tracks in “%1”: %2. Downloading them takes a long time and much disk space.")
                .replace("%2", () => dialog.backend.downloadAsk)
                .replace("%1", () => dialog.backend.downloadAskTitle)
            color: dialog.theme.dim
            wrapMode: Text.Wrap
        }
        RowLayout {
            Layout.fillWidth: true
            Layout.topMargin: 8
            spacing: 8

            Item { Layout.fillWidth: true }
            TextButton {
                theme: dialog.theme
                text: dialog.theme.tr("Cancel")
                onClicked: dialog.backend.answerDownload(false)
            }
            TextButton {
                theme: dialog.theme
                text: dialog.theme.tr("Download")
                accent: true
                onClicked: dialog.backend.answerDownload(true)
            }
        }
    }
}
