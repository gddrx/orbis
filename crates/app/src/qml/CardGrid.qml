import QtQuick
import QtQuick.Controls.Basic

// The library's playlists as a grid. A click opens the playlist in its tab;
// middle click or Ctrl+click opens it in the background.
GridView {
    id: view

    required property var backend
    required property var covers
    required property var theme

    readonly property int cardSize: 160

    clip: true
    reuseItems: true
    boundsBehavior: Flickable.StopAtBounds
    leftMargin: 14
    rightMargin: 14
    topMargin: 14
    bottomMargin: 14
    cellWidth: cardSize + 22
    cellHeight: cardSize + 62

    delegate: Item {
        id: card

        required property string title
        required property string subtitle
        required property string target_id
        required property string thumbnail

        width: view.cellWidth
        height: view.cellHeight

        Cover {
            id: picture
            width: view.cardSize
            height: view.cardSize
            covers: view.covers
            url: card.thumbnail
        }
        Label {
            id: name
            anchors.top: picture.bottom
            anchors.topMargin: 6
            width: view.cardSize
            text: card.title
            color: hover.hovered ? view.theme.accent : view.theme.text
            elide: Text.ElideRight
        }
        Label {
            anchors.top: name.bottom
            anchors.topMargin: 2
            width: view.cardSize
            text: card.subtitle
            color: view.theme.dim
            font.pixelSize: 12
            elide: Text.ElideRight
        }

        HoverHandler { id: hover }
        TapHandler {
            acceptedButtons: Qt.LeftButton
            acceptedModifiers: Qt.NoModifier
            onTapped: view.backend.openCard("playlist", card.target_id, card.title, true)
        }
        TapHandler {
            acceptedButtons: Qt.LeftButton
            acceptedModifiers: Qt.ControlModifier
            onTapped: view.backend.openCard("playlist", card.target_id, card.title, false)
        }
        TapHandler {
            acceptedButtons: Qt.MiddleButton
            onTapped: view.backend.openCard("playlist", card.target_id, card.title, false)
        }
    }

    Loading {
        anchors.centerIn: parent
        active: view.backend.loading
        tint: view.theme.dim
    }

    ScrollBar.vertical: ScrollBar {}
}
