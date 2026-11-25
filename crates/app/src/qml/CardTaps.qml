import QtQuick

// What a card or a track row does with the mouse: a click opens it (a track
// plays its radio), Ctrl+click and middle click open it in the background,
// right click on a track shows the track menu. Fills the item it is put in.
Item {
    id: taps

    required property var backend
    // The card: `kind`, `id`, `title`, …
    required property var card
    required property var trackMenu
    readonly property bool hovered: hover.hovered

    anchors.fill: parent

    HoverHandler { id: hover }
    TapHandler {
        acceptedButtons: Qt.LeftButton
        acceptedModifiers: Qt.NoModifier
        onTapped: taps.backend.openCard(taps.card.kind, taps.card.id, taps.card.title, true)
    }
    TapHandler {
        acceptedButtons: Qt.LeftButton
        acceptedModifiers: Qt.ControlModifier
        onTapped: taps.backend.openCard(taps.card.kind, taps.card.id, taps.card.title, false)
    }
    TapHandler {
        acceptedButtons: Qt.MiddleButton
        onTapped: taps.backend.openCard(taps.card.kind, taps.card.id, taps.card.title, false)
    }
    TapHandler {
        acceptedButtons: Qt.RightButton
        enabled: taps.card.kind === "track"
        onTapped: {
            taps.backend.menuForCard(taps.card.id, taps.card.title, taps.card.subtitle, taps.card.thumbnail)
            taps.trackMenu.popup()
        }
    }
}
