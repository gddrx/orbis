import QtQuick
import QtQuick.Controls.Basic

// A drop-down list with rounded corners on the dark ground.
ComboBox {
    id: box

    required property var theme

    implicitHeight: 34
    leftPadding: 12

    background: Rectangle {
        implicitWidth: 140
        implicitHeight: 34
        radius: box.theme.radius
        color: box.down || box.hovered ? box.theme.controlHover : box.theme.control
        border.width: box.visualFocus ? 1 : 0
        border.color: box.theme.accent
    }
    popup.background: Rectangle {
        color: box.theme.raised
        radius: box.theme.radius
        border.width: 1
        border.color: box.theme.control
    }
}
