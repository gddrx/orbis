import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Controls.impl
import QtQuick.Layouts

// The tabs opened since the start of the account — playlists, albums,
// artists, searches, radios — in a column at the left; the pinned ones
// (home, the mixes, the library) stay in the strip at the top. Click shows,
// middle click or × closes, drag moves; ▶ marks the tab that plays.
Rectangle {
    id: side

    required property var model
    required property var backend
    required property var theme

    color: theme.surface

    ListView {
        id: list

        anchors.fill: parent
        anchors.leftMargin: 8
        anchors.rightMargin: 8
        anchors.topMargin: 4
        clip: true
        model: side.model
        boundsBehavior: Flickable.StopAtBounds

        // The gap between the tabs is inside each: the pinned tabs take no
        // room here, gap included.
        delegate: Item {
            id: tab

            required property int index
            required property var tab_id
            required property string title
            required property string kind
            required property bool pinned
            required property bool playing
            required property bool shown

            visible: !tab.pinned
            width: ListView.view.width
            height: tab.pinned ? 0 : 42

            Rectangle {
                anchors.fill: parent
                anchors.bottomMargin: 4
                radius: side.theme.radius
                color: tab.shown ? side.theme.selected
                    : (hover.hovered ? side.theme.hover : "transparent")

                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: 12
                    anchors.rightMargin: 4
                    spacing: 6

                    IconImage {
                        visible: tab.playing
                        Layout.preferredWidth: 14
                        Layout.preferredHeight: 14
                        sourceSize.width: 14
                        sourceSize.height: 14
                        source: Qt.resolvedUrl("icons/play.svg")
                        color: side.theme.accent
                    }
                    Label {
                        id: name

                        Layout.fillWidth: true
                        text: tab.title
                        color: tab.shown ? side.theme.text : side.theme.dim
                        elide: Text.ElideRight
                    }
                    // Seen on the shown tab and under the pointer; it keeps
                    // its room otherwise, so the name does not move.
                    IconButton {
                        implicitWidth: 26
                        implicitHeight: 26
                        opacity: hover.hovered || tab.shown ? 1 : 0
                        enabled: opacity > 0
                        theme: side.theme
                        iconName: "close"
                        iconSize: 14
                        tip: side.theme.tr("Close")
                        onClicked: side.backend.closeTab(tab.tab_id)
                    }
                }
            }

            ToolTip.visible: hover.hovered && name.truncated
            ToolTip.delay: 600
            ToolTip.text: tab.title

            HoverHandler { id: hover }
            TapHandler {
                acceptedButtons: Qt.LeftButton
                onTapped: side.backend.showTab(tab.tab_id)
            }
            TapHandler {
                acceptedButtons: Qt.MiddleButton
                onTapped: side.backend.closeTab(tab.tab_id)
            }
            DragHandler {
                target: null
                xAxis.enabled: false
                onActiveChanged: {
                    if (active)
                        return
                    const point = tab.mapToItem(list.contentItem, tab.width / 2, centroid.position.y)
                    const to = list.indexAt(point.x, point.y)
                    if (to >= 0 && to !== tab.index)
                        side.backend.moveTab(tab.tab_id, to)
                }
            }
        }

        ScrollBar.vertical: ScrollBar {}
    }
}
