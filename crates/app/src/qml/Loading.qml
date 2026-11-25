import QtQuick

// The mark of a page that loads, in its middle: it shows only once the
// load has taken longer than a moment, so a quick one draws nothing, and
// it goes at once when the load ends. The Basic style's busy indicator
// fades out instead, and the window draws at full rate while it does.
Spinner {
    id: loading

    // A load is under way.
    required property bool active
    property bool late: false

    width: 40
    height: 40
    visible: active && late
    onActiveChanged: if (!active) late = false

    Timer {
        interval: 400
        running: loading.active && !loading.late
        onTriggered: loading.late = true
    }
}
