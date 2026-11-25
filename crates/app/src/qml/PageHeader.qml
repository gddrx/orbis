import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts

// The top of an album, artist, playlist, radio or search page: its picture,
// name and details, "download all" for an album or a playlist, and what a
// search shows: everything, or only songs, videos, albums or artists. Empty
// on the home page and the library.
Item {
    id: header

    required property var backend
    required property var covers
    required property var theme

    readonly property bool search: backend.shownKind === "search"

    height: backend.pageTitle.length === 0 ? 0
        : (backend.pageThumbnail.length > 0 ? 164 : (search ? 132 : 92))
    visible: height > 0

    RowLayout {
        anchors.fill: parent
        anchors.margins: 18
        spacing: 18

        Cover {
            visible: url.length > 0
            Layout.preferredWidth: 128
            Layout.preferredHeight: 128
            covers: header.covers
            url: header.backend.pageThumbnail
        }
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 6
            Label {
                Layout.fillWidth: true
                text: header.backend.pageTitle
                color: header.theme.text
                font.pixelSize: 24
                font.bold: true
                wrapMode: Text.Wrap
                maximumLineCount: 2
                elide: Text.ElideRight
            }
            Label {
                Layout.fillWidth: true
                text: header.backend.pageSubtitle
                color: header.theme.dim
                elide: Text.ElideRight
            }
            Row {
                visible: header.search
                spacing: 6

                Repeater {
                    model: [
                        { name: "", text: "All" },
                        { name: "songs", text: "Songs" },
                        { name: "videos", text: "Videos" },
                        { name: "albums", text: "Albums" },
                        { name: "artists", text: "Artists" }
                    ]

                    delegate: TextButton {
                        required property var modelData

                        theme: header.theme
                        accent: header.backend.searchFilter === modelData.name
                        text: header.theme.tr(modelData.text)
                        onClicked: header.backend.setSearchFilter(modelData.name)
                    }
                }
            }
        }
        // A whole album or playlist; mixes and radios never end.
        TextButton {
            Layout.alignment: Qt.AlignTop
            visible: header.backend.shownDownloadable
            theme: header.theme
            // A long list is read first; then the backend may ask.
            enabled: !header.backend.downloadReading
            text: header.backend.downloadReading ? header.theme.tr("Reading the list…")
                                                 : header.theme.tr("Download all")
            onClicked: header.backend.downloadShown()
        }
    }
}
