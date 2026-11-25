import QtQuick
import QtQuick.Controls.Basic

// A text field with rounded corners; the accent outlines it while typing.
TextField {
    id: field

    required property var theme

    implicitHeight: 34
    leftPadding: 12
    rightPadding: 12
    topPadding: 0
    bottomPadding: 0
    verticalAlignment: TextInput.AlignVCenter
    color: theme.text
    placeholderTextColor: theme.faint
    selectionColor: theme.accent
    selectedTextColor: theme.background
    selectByMouse: true

    background: Rectangle {
        implicitWidth: 200
        implicitHeight: 34
        radius: field.theme.radius
        color: field.activeFocus || field.hovered ? field.theme.controlHover : field.theme.control
        border.width: field.activeFocus ? 1 : 0
        border.color: field.theme.accent
    }
}
