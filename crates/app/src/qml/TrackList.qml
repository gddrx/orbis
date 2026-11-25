import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts

// The tracks of the shown tab under the page's header: click plays from
// there, right click shows the track menu, the thumbs rate a track.
ListView {
    id: view

    required property var backend
    required property var covers
    required property var theme
    required property var trackMenu

    // The track asked to play, read here once: delegates must not read the
    // backend while it resets the model (reading it then re-enters it, and
    // Qt Bridge for Rust crashes the window).
    readonly property string startingId: backend.starting

    clip: true
    reuseItems: true
    boundsBehavior: Flickable.StopAtBounds

    // A second after the list comes to rest, the links of the rows in view
    // are fetched ahead (the backend limits how many): a click there then
    // plays at once.
    onContentYChanged: rest.restart()
    onHeightChanged: rest.restart()
    onCountChanged: rest.restart()
    Component.onCompleted: rest.restart()

    Timer {
        id: rest
        interval: 1000
        onTriggered: {
            let first = -1
            let last = -1
            for (let y = view.contentY; y < view.contentY + view.height; y += 28) {
                const index = view.indexAt(view.width / 2, y)
                if (index < 0)
                    continue
                if (first < 0 || index < first)
                    first = index
                if (index > last)
                    last = index
            }
            if (first >= 0)
                view.backend.prefetchVisible(first, last)
        }
    }

    header: PageHeader {
        width: ListView.view.width
        backend: view.backend
        covers: view.covers
        theme: view.theme
    }

    delegate: Rectangle {
        id: row

        required property int index
        required property string video_id
        required property string title
        required property string artists
        required property string duration
        required property string thumbnail
        required property bool current
        // `like`, `dislike` or empty.
        required property string rating

        readonly property bool starting: view.startingId === video_id && !current

        width: ListView.view.width
        height: 56
        color: current ? view.theme.selected
            : (hover.hovered || starting ? view.theme.hover : "transparent")

        // A track under the pointer for a moment is likely to be clicked:
        // its link is fetched ahead.
        Timer {
            interval: 250
            running: hover.hovered && !row.current
            onTriggered: view.backend.prefetchRow(row.index)
        }

        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 14
            anchors.rightMargin: 14
            spacing: 12

            Cover {
                Layout.preferredWidth: 40
                Layout.preferredHeight: 40
                covers: view.covers
                url: row.thumbnail

                Rectangle {
                    anchors.fill: parent
                    visible: row.starting
                    color: "#99000000"

                    Spinner {
                        anchors.centerIn: parent
                        width: 26
                        height: 26
                        visible: row.starting
                    }
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2
                Label {
                    Layout.fillWidth: true
                    text: row.title
                    color: row.current ? view.theme.accent : view.theme.text
                    font.bold: row.current
                    elide: Text.ElideRight
                }
                Label {
                    Layout.fillWidth: true
                    text: row.artists
                    color: view.theme.dim
                    font.pixelSize: 12
                    elide: Text.ElideRight
                }
            }
            IconButton {
                implicitWidth: 32
                implicitHeight: 32
                theme: view.theme
                iconSize: 18
                iconName: row.rating === "like" ? "thumb-up-filled" : "thumb-up"
                iconColor: row.rating === "like" ? view.theme.accent
                    : (hover.hovered ? view.theme.dim : view.theme.faint)
                hoverIconColor: row.rating === "like" ? view.theme.accent : view.theme.text
                tip: row.rating === "like" ? view.theme.tr("Remove from liked") : view.theme.tr("Like")
                onClicked: view.backend.rateRow(row.index, "like")
            }
            IconButton {
                implicitWidth: 32
                implicitHeight: 32
                theme: view.theme
                iconSize: 18
                iconName: row.rating === "dislike" ? "thumb-down-filled" : "thumb-down"
                iconColor: row.rating === "dislike" ? view.theme.text
                    : (hover.hovered ? view.theme.dim : view.theme.faint)
                tip: row.rating === "dislike" ? view.theme.tr("Remove dislike") : view.theme.tr("Dislike")
                onClicked: view.backend.rateRow(row.index, "dislike")
            }
            Label {
                Layout.preferredWidth: 44
                horizontalAlignment: Text.AlignRight
                text: row.duration
                color: view.theme.faint
                font.features: { "tnum": 1 }
            }
        }

        HoverHandler { id: hover }
        TapHandler {
            acceptedButtons: Qt.LeftButton
            onTapped: view.backend.playTrack(row.index)
        }
        TapHandler {
            acceptedButtons: Qt.RightButton
            onTapped: {
                view.backend.menuForRow(row.index)
                view.trackMenu.popup()
            }
        }
    }

    Loading {
        anchors.centerIn: parent
        active: view.backend.loading
        tint: view.theme.dim
    }

    ScrollBar.vertical: ScrollBar {}
}
