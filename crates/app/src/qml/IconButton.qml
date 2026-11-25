import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Controls.impl

// A round button with an icon from icons/: the icon is tinted and sits in
// the exact middle; the ground shows on hover and press only, unless the
// button is `filled` (the play button).
ToolButton {
    id: button

    required property var theme
    // A file name in icons/ without ".svg".
    property string iconName: ""
    property int iconSize: 20
    property color iconColor: theme.dim
    property color hoverIconColor: theme.text
    // A tooltip, or empty.
    property string tip: ""
    property bool filled: false
    property color fillColor: theme.text

    implicitWidth: 36
    implicitHeight: 36
    padding: 0
    focusPolicy: Qt.NoFocus
    display: AbstractButton.IconOnly
    icon.source: iconName.length > 0 ? Qt.resolvedUrl("icons/" + iconName + ".svg") : ""
    icon.width: iconSize
    icon.height: iconSize
    icon.color: !enabled ? theme.faint : (hovered && !filled ? hoverIconColor : iconColor)

    ToolTip.visible: tip.length > 0 && hovered
    ToolTip.delay: 600
    ToolTip.text: tip

    contentItem: Item {
        IconImage {
            anchors.centerIn: parent
            width: button.icon.width
            height: button.icon.height
            sourceSize.width: button.icon.width
            sourceSize.height: button.icon.height
            source: button.icon.source
            color: button.icon.color
        }
    }

    background: Rectangle {
        implicitWidth: button.implicitWidth
        implicitHeight: button.implicitHeight
        radius: Math.min(width, height) / 2
        color: button.filled
            ? (button.down ? Qt.darker(button.fillColor, 1.25)
               : button.hovered ? Qt.lighter(button.fillColor, 1.08) : button.fillColor)
            : (button.down ? button.theme.pressed
               : button.hovered ? button.theme.hover : "transparent")
        opacity: button.enabled ? 1 : 0.5
    }
}
