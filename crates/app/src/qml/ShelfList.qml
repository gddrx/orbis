import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts

// Shelves of cards: the home page, artist pages and search results. A shelf
// is a carousel, or tracks as rows. A click opens a playlist, album or
// artist in its tab or plays a track's radio; middle click or Ctrl+click
// opens it in the background; right click on a track shows the track menu.
// The next home page loads as the end comes near.
ListView {
    id: view

    required property var backend
    required property var covers
    required property var theme
    required property var trackMenu

    readonly property int cardSize: 160
    // Distance from the bottom of the view to the end of the content.
    readonly property real toEnd: originY + contentHeight - contentY - height
    // The backend's word, read when it changes: bindings that follow the
    // scrolling read this, never the backend, which may be busy resetting
    // the model as the content moves (reading it then re-enters it, and
    // Qt Bridge for Rust crashes the window).
    readonly property bool hasMore: backend.hasMore

    // Where each tab's shelves were scrolled, by tab ID. The model holds one
    // tab's shelves at a time and is reset for another tab, which would
    // start that tab at the top again when it is shown later.
    property var places: ({})
    // The tab whose shelves the model holds, -1 before the first.
    property int placeOf: -1

    function loadMoreIfNear() {
        if (visible && backend.hasMore && toEnd < 800)
            backend.loadMore()
    }

    // The shelf at the top of the view and how far into it, or the header
    // (index -1) and how far into the content; or the end. A shelf, not
    // pixels from the top: shelves out of view have no height yet, only an
    // estimate, and so has the content.
    function place() {
        const index = indexAt(0, contentY)
        const item = index >= 0 ? itemAtIndex(index) : null
        return {
            index: item ? index : -1,
            offset: item ? contentY - item.y : contentY - originY,
            atEnd: toEnd < 1 && contentY > originY
        }
    }

    // Back to where the tab now in the model was left; a tab not seen
    // before stays at the top, where the reset put it.
    function restorePlace() {
        placeOf = backend.shelvesTab
        const saved = places[placeOf]
        if (!saved || count === 0)
            return
        if (saved.atEnd) {
            positionViewAtEnd()
            toTheEnd.tries = 5
            toTheEnd.restart()
            return
        }
        let to = originY + saved.offset
        if (saved.index >= 0 && saved.index < count) {
            positionViewAtIndex(saved.index, ListView.Beginning)
            const item = itemAtIndex(saved.index)
            if (item)
                to = item.y + saved.offset
        }
        contentY = Math.max(originY, Math.min(to, originY + contentHeight - height))
    }

    // The end as positionViewAtEnd() finds it is an estimate: the shelves
    // there are laid out only then, and the end may move. Again a few times,
    // a tenth of a second apart, until the view is at the end.
    Timer {
        id: toTheEnd

        property int tries: 0

        interval: 100
        repeat: true
        onTriggered: {
            if (view.toEnd < 1 || tries <= 0) {
                stop()
                return
            }
            tries -= 1
            view.positionViewAtEnd()
        }
    }

    clip: true
    reuseItems: true
    boundsBehavior: Flickable.StopAtBounds
    // Later, not at once: the size changes while the backend fills the
    // model, and the bridge does not let QML into the backend then.
    onToEndChanged: Qt.callLater(loadMoreIfNear)
    onVisibleChanged: Qt.callLater(loadMoreIfNear)

    header: PageHeader {
        width: ListView.view.width
        backend: view.backend
        covers: view.covers
        theme: view.theme
    }

    delegate: Column {
        id: shelf

        required property string title
        // `carousel` or `list`.
        required property string layout
        required property var cards

        readonly property bool carousel: layout === "carousel"

        width: ListView.view.width
        // A later page of a filtered search goes on under the first one's
        // title, without its own.
        topPadding: title.length > 0 ? 14 : 0
        spacing: title.length > 0 ? 8 : 0

        RowLayout {
            visible: shelf.title.length > 0
            width: parent.width
            spacing: 2

            Label {
                Layout.fillWidth: true
                Layout.leftMargin: 14
                text: shelf.title
                color: view.theme.text
                font.pixelSize: 17
                font.bold: true
                elide: Text.ElideRight
            }
            IconButton {
                visible: shelf.carousel && !(row.atXBeginning && row.atXEnd)
                enabled: !row.atXBeginning
                theme: view.theme
                iconName: "chevron-left"
                iconSize: 24
                iconColor: view.theme.text
                onClicked: row.page(-1)
            }
            IconButton {
                Layout.rightMargin: 10
                visible: shelf.carousel && !(row.atXBeginning && row.atXEnd)
                enabled: !row.atXEnd
                theme: view.theme
                iconName: "chevron-right"
                iconSize: 24
                iconColor: view.theme.text
                onClicked: row.page(1)
            }
        }

        // A carousel of cards.
        ListView {
            id: row

            // Scrolls by a view's width less a card, so one card stays in sight.
            function page(direction) {
                const last = contentWidth + rightMargin - width
                const to = contentX + direction * (width - view.cardSize)
                contentX = Math.max(-leftMargin, Math.min(to, last))
            }

            visible: shelf.carousel
            width: parent.width
            height: visible ? view.cardSize + 52 : 0
            orientation: ListView.Horizontal
            spacing: 14
            leftMargin: 14
            rightMargin: 14
            boundsBehavior: Flickable.StopAtBounds
            reuseItems: true
            model: shelf.carousel ? shelf.cards : []
            onModelChanged: positionViewAtBeginning()

            delegate: Item {
                id: card

                required property var modelData

                width: modelData.wide ? Math.round(view.cardSize * 16 / 9) : view.cardSize
                height: row.height

                Cover {
                    id: picture
                    width: parent.width
                    height: view.cardSize
                    covers: view.covers
                    url: card.modelData.thumbnail
                }
                Label {
                    id: name
                    anchors.top: picture.bottom
                    anchors.topMargin: 6
                    width: parent.width
                    text: card.modelData.title
                    color: cardTaps.hovered ? view.theme.accent : view.theme.text
                    elide: Text.ElideRight
                }
                Label {
                    anchors.top: name.bottom
                    anchors.topMargin: 2
                    width: parent.width
                    text: card.modelData.subtitle
                    color: view.theme.dim
                    font.pixelSize: 12
                    elide: Text.ElideRight
                }

                CardTaps {
                    id: cardTaps
                    backend: view.backend
                    card: card.modelData
                    trackMenu: view.trackMenu
                }
            }
        }

        // Tracks one under another (search results). A few dozen at most,
        // so a plain Repeater.
        Column {
            id: list

            visible: !shelf.carousel
            width: parent.width

            Repeater {
                model: shelf.carousel ? [] : shelf.cards

                delegate: Rectangle {
                    id: track

                    required property var modelData

                    width: list.width
                    height: 56
                    color: trackTaps.hovered ? view.theme.hover : "transparent"

                    RowLayout {
                        anchors.fill: parent
                        anchors.leftMargin: 14
                        anchors.rightMargin: 18
                        spacing: 12

                        Cover {
                            Layout.preferredWidth: track.modelData.wide ? 78 : 44
                            Layout.preferredHeight: 44
                            covers: view.covers
                            url: track.modelData.thumbnail
                        }
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2
                            Label {
                                Layout.fillWidth: true
                                text: track.modelData.title
                                color: view.theme.text
                                elide: Text.ElideRight
                            }
                            Label {
                                Layout.fillWidth: true
                                text: track.modelData.subtitle
                                color: view.theme.dim
                                font.pixelSize: 12
                                elide: Text.ElideRight
                            }
                        }
                        Label {
                            text: track.modelData.duration
                            color: view.theme.faint
                        }
                    }

                    CardTaps {
                        id: trackTaps
                        backend: view.backend
                        card: track.modelData
                        trackMenu: view.trackMenu
                    }
                }
            }
        }
    }

    footer: Item {
        id: footer

        width: ListView.view.width
        height: view.backend.hasMore ? 64 : 18
        // More is on its way: turns only while the end of the page is in
        // view. Further up it would keep the window drawing for nothing.
        Spinner {
            anchors.centerIn: parent
            width: 32
            height: 32
            tint: view.theme.dim
            visible: view.hasMore && view.toEnd < footer.height
        }
    }

    Loading {
        anchors.centerIn: parent
        active: view.backend.loading
        tint: view.theme.dim
    }

    ScrollBar.vertical: ScrollBar {}

    // The smoke test: down by a third of the view every 300 ms, waiting for
    // more pages at the end, until there are none.
    Timer {
        id: smokeScroll

        property int steps: 0

        interval: 300
        repeat: true
        onTriggered: {
            if (view.toEnd < 1 && !view.backend.hasMore) {
                stop()
                view.backend.smokeNote("home page scrolled to the end in " + steps + " steps")
                view.backend.smokeScrolled()
                return
            }
            if (view.toEnd >= 1) {
                view.contentY += Math.min(view.toEnd, view.height / 3)
                steps++
                view.backend.smokeNote("scrolled to " + Math.round(view.contentY - view.originY + view.height)
                                       + " of " + Math.round(view.contentHeight) + " px")
            }
        }
    }

    Connections {
        target: view.model
        function onModelAboutToBeReset() {
            if (view.placeOf >= 0)
                view.places[view.placeOf] = view.place()
        }
        // Later: the backend is still filling the model and does not let
        // QML in.
        function onModelReset() { Qt.callLater(view.restorePlace) }
    }

    // The smoke test: where the view is, after moving it a little into
    // shelf `smokeShelf` unless that is -1.
    property int smokeShelf: -1
    function notePlace() {
        if (smokeShelf >= 0 && smokeShelf < count) {
            positionViewAtIndex(smokeShelf, ListView.Beginning)
            contentY += 37
        }
        smokeShelf = -1
        const now = place()
        backend.smokePlace(now.index, now.offset, now.atEnd)
    }

    Connections {
        target: view.backend
        function onMoreChanged() { Qt.callLater(view.loadMoreIfNear) }
        function onScrollRequested() {
            if (view.visible)
                smokeScroll.start()
        }
        function onPlaceRequested(shelf) {
            if (view.visible) {
                view.smokeShelf = shelf
                Qt.callLater(view.notePlace)
            }
        }
    }
}
