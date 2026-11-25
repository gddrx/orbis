import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Controls.impl
import QtQuick.Layouts

// The pinned tabs (home, the mixes, the library): click shows, drag moves;
// ▶ marks the tab that plays. The tabs opened since are in the column at the
// left (TabSidebar.qml). The search field at the right opens results in a
// tab, and while it is typed in, YouTube Music's suggestions show under it.
Rectangle {
    id: strip

    required property var model
    required property var backend
    required property var theme

    // The ⚙ button was clicked.
    signal settingsClicked()

    function focusSearch() {
        search.forceActiveFocus()
        search.selectAll()
    }

    // Searches `query` (typed or a suggestion) and leaves the field.
    function searchFor(query) {
        typed.stop()
        strip.backend.clearSuggestions()
        search.text = query
        search.focus = false
        strip.backend.search(query)
    }

    // The smoke test types and chooses as a user would. Later: the backend
    // is busy while it asks.
    Connections {
        target: strip.backend
        function onTypingRequested(text) {
            Qt.callLater(() => {
                search.forceActiveFocus()
                search.text = text
                typed.restart()
            })
        }
        function onSuggestionRequested(index) {
            Qt.callLater(() => {
                const suggestions = strip.backend.searchSuggestions
                if (index >= 0 && index < suggestions.length)
                    strip.searchFor(suggestions[index])
            })
        }
    }

    // Suggestions are asked for 200 ms after the last key.
    Timer {
        id: typed
        interval: 200
        onTriggered: strip.backend.suggest(search.text)
    }

    implicitHeight: 48
    color: theme.surface

    IconButton {
        id: gear

        anchors.right: parent.right
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        theme: strip.theme
        iconName: "settings"
        tip: strip.theme.tr("Settings")
        onClicked: strip.settingsClicked()
    }

    Field {
        id: search

        anchors.right: gear.left
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        width: 260
        theme: strip.theme
        leftPadding: 34
        placeholderText: strip.theme.tr("Search")
        onTextEdited: typed.restart()
        onActiveFocusChanged: if (!activeFocus) {
            typed.stop()
            strip.backend.clearSuggestions()
        }
        onAccepted: strip.searchFor(choices.currentIndex >= 0 && suggestions.opened
            ? strip.backend.searchSuggestions[choices.currentIndex] : text)
        Keys.onDownPressed: choices.currentIndex = Math.min(choices.currentIndex + 1, choices.count - 1)
        Keys.onUpPressed: choices.currentIndex = Math.max(choices.currentIndex - 1, -1)
        Keys.onEscapePressed: {
            text = ""
            focus = false
        }

        // What YouTube Music suggests for the text so far: a click or
        // Enter on one searches it.
        Popup {
            id: suggestions

            y: search.height + 4
            width: search.width
            padding: 4
            visible: search.activeFocus && strip.backend.searchSuggestions.length > 0
            closePolicy: Popup.NoAutoClose
            background: Rectangle {
                color: strip.theme.raised
                radius: strip.theme.radius
                border.width: 1
                border.color: strip.theme.control
            }

            contentItem: ListView {
                id: choices

                implicitHeight: contentHeight
                currentIndex: -1
                interactive: false
                model: strip.backend.searchSuggestions
                onModelChanged: currentIndex = -1

                delegate: ItemDelegate {
                    id: choice

                    required property string modelData
                    required property int index

                    width: ListView.view.width
                    height: 32
                    // The field keeps the focus: losing it drops the list.
                    focusPolicy: Qt.NoFocus
                    text: modelData
                    highlighted: ListView.isCurrentItem
                    palette.text: strip.theme.text
                    background: Rectangle {
                        radius: strip.theme.radius
                        color: choice.highlighted || choice.hovered ? strip.theme.hover : "transparent"
                    }
                    onClicked: strip.searchFor(modelData)
                }
            }
        }

        IconImage {
            anchors.left: parent.left
            anchors.leftMargin: 10
            anchors.verticalCenter: parent.verticalCenter
            width: 18
            height: 18
            sourceSize.width: 18
            sourceSize.height: 18
            source: Qt.resolvedUrl("icons/search.svg")
            color: strip.theme.faint
        }
    }

    ListView {
        id: list

        anchors.top: parent.top
        anchors.bottom: parent.bottom
        anchors.left: parent.left
        anchors.right: search.left
        anchors.leftMargin: 8
        anchors.rightMargin: 8
        orientation: ListView.Horizontal
        // The gap between the tabs is inside each: the tabs of the column
        // at the left take no room here, gap included.
        spacing: 0
        clip: true
        model: strip.model
        boundsBehavior: Flickable.StopAtBounds

        // The delegate is as tall as the strip: a horizontal list puts its
        // items at the top, so the pill is centred inside.
        delegate: Item {
            id: tab

            required property int index
            required property var tab_id
            required property string title
            required property string kind
            required property bool pinned
            required property bool playing
            required property bool shown

            visible: tab.pinned
            width: tab.pinned ? Math.min(240, row.implicitWidth + 28) + 4 : 0
            height: ListView.view.height

            Rectangle {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.rightMargin: 4
                anchors.verticalCenter: parent.verticalCenter
                height: 34
                radius: strip.theme.radius
                color: tab.shown ? strip.theme.selected
                    : (hover.hovered ? strip.theme.hover : "transparent")

                RowLayout {
                    id: row

                    anchors.left: parent.left
                    anchors.leftMargin: 14
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 6

                    IconImage {
                        visible: tab.playing
                        Layout.preferredWidth: 14
                        Layout.preferredHeight: 14
                        Layout.alignment: Qt.AlignVCenter
                        sourceSize.width: 14
                        sourceSize.height: 14
                        source: Qt.resolvedUrl("icons/play.svg")
                        color: strip.theme.accent
                    }
                    Label {
                        Layout.alignment: Qt.AlignVCenter
                        Layout.maximumWidth: 170
                        text: tab.title
                        color: tab.shown ? strip.theme.text : strip.theme.dim
                        elide: Text.ElideRight
                    }
                }
            }

            HoverHandler { id: hover }
            TapHandler {
                acceptedButtons: Qt.LeftButton
                onTapped: strip.backend.showTab(tab.tab_id)
            }
            DragHandler {
                target: null
                yAxis.enabled: false
                onActiveChanged: {
                    if (active)
                        return
                    const point = tab.mapToItem(list.contentItem, centroid.position.x, tab.height / 2)
                    const to = list.indexAt(point.x, point.y)
                    if (to >= 0 && to !== tab.index)
                        strip.backend.moveTab(tab.tab_id, to)
                }
            }
        }
    }
}
