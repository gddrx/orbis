import QtQuick
import QtQuick.Controls.impl

// A turning arc: a track is being started. It turns only while shown.
IconImage {
    id: spinner

    property color tint: "white"

    sourceSize.width: width
    sourceSize.height: height
    source: Qt.resolvedUrl("icons/spinner.svg")
    color: tint

    RotationAnimator on rotation {
        running: spinner.visible
        from: 0
        to: 360
        duration: 900
        loops: Animation.Infinite
    }
}
