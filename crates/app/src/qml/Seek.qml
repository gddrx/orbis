import QtQuick
import QtQuick.Controls.Basic

// A slider with a thin rounded groove, the passed part in the accent and a
// small handle that grows under the pointer: the progress and the volume.
Slider {
    id: slider

    required property var theme

    implicitHeight: 24
    padding: 0
    focusPolicy: Qt.NoFocus

    background: Rectangle {
        x: slider.leftPadding
        y: slider.topPadding + (slider.availableHeight - height) / 2
        width: slider.availableWidth
        height: 4
        radius: 2
        color: slider.theme.control

        Rectangle {
            width: slider.visualPosition * parent.width
            height: parent.height
            radius: 2
            color: slider.enabled ? slider.theme.accent : slider.theme.faint
        }
    }

    handle: Rectangle {
        readonly property real size: slider.pressed || slider.hovered ? 14 : 10

        x: slider.leftPadding + slider.visualPosition * (slider.availableWidth - width)
        y: slider.topPadding + (slider.availableHeight - height) / 2
        width: size
        height: size
        radius: size / 2
        color: slider.theme.text
        visible: slider.enabled
    }
}
