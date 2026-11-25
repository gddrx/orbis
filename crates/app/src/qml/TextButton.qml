import QtQuick
import QtQuick.Controls.Basic

// A button with text and rounded corners. `accent` fills it with the accent
// color (the main action), `danger` with red; `flat` shows the ground only
// on hover.
Button {
    id: button

    required property var theme
    property bool accent: false
    property bool danger: false

    readonly property color fill: danger ? theme.danger : (accent ? theme.accent : theme.control)

    implicitHeight: 34
    leftPadding: 14
    rightPadding: 14
    topPadding: 0
    bottomPadding: 0
    focusPolicy: Qt.TabFocus

    contentItem: Text {
        text: button.text
        font: button.font
        color: !button.enabled ? button.theme.faint
            : (button.accent || button.danger) ? button.theme.background : button.theme.text
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
    }

    background: Rectangle {
        implicitWidth: 80
        implicitHeight: 34
        radius: button.theme.radius
        color: button.flat && !button.hovered && !button.down ? "transparent"
            : button.down ? Qt.darker(button.fill, 1.2)
            : button.hovered ? ((button.accent || button.danger) ? Qt.lighter(button.fill, 1.1)
                                                                : button.theme.controlHover)
            : button.fill
        opacity: button.enabled ? 1 : 0.6
        border.width: button.visualFocus ? 1 : 0
        border.color: button.theme.accent
    }
}
