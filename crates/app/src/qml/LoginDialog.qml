import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts

// Signing in to YouTube Music: through the Google window — the player's own
// session, the main way — or with the sign-in of a browser.
Popup {
    id: dialog

    required property var backend
    required property var theme

    // The browsers orbis can take a sign-in from; offered are those that
    // have a cookie database, which the backend looks for as the dialog opens.
    readonly property var allBrowsers: [
        { text: "Google Chrome", value: "chrome" },
        { text: "Chromium", value: "chromium" },
        { text: "Brave", value: "brave" },
        { text: "Vivaldi", value: "vivaldi" },
        { text: "Firefox", value: "firefox" }
    ]
    readonly property var browsers: allBrowsers.filter(
        b => dialog.backend.signInBrowsers.indexOf(b.value) >= 0)

    // Later: the dialog may open while the backend tells that the session
    // was refused, and the backend does not let QML in then.
    onAboutToShow: Qt.callLater(dialog.backend.findBrowsers)

    anchors.centerIn: Overlay.overlay
    width: 440
    modal: true
    focus: true
    padding: 20
    closePolicy: dialog.backend.signingIn ? Popup.NoAutoClose : Popup.CloseOnEscape
    background: Rectangle {
        color: dialog.theme.raised
        radius: 10
    }

    contentItem: ColumnLayout {
        spacing: 8

        Label {
            text: dialog.theme.tr("Sign in to YouTube Music")
            color: dialog.theme.text
            font.pixelSize: 18
            font.bold: true
        }
        Label {
            Layout.fillWidth: true
            text: dialog.theme.tr("Without signing in there is no library, no mixes and no ratings.")
            color: dialog.theme.dim
            wrapMode: Text.Wrap
        }

        TextButton {
            Layout.fillWidth: true
            Layout.topMargin: 8
            theme: dialog.theme
            text: dialog.theme.tr("Sign in with a Google window")
            enabled: !dialog.backend.signingIn
            // The main way stands out in the accent color.
            accent: true
            onClicked: dialog.backend.signInWindow()
        }
        Label {
            Layout.fillWidth: true
            text: dialog.theme.tr("A separate Google window opens. The sign-in belongs to orbis alone; your browser is not touched.")
            color: dialog.theme.faint
            font.pixelSize: 12
            wrapMode: Text.Wrap
        }

        Label {
            Layout.topMargin: 10
            text: dialog.theme.tr("Or take the sign-in from a browser")
            color: dialog.theme.text
        }
        RowLayout {
            Layout.fillWidth: true
            visible: dialog.browsers.length > 0
            spacing: 8

            Choice {
                id: browser

                theme: dialog.theme

                Layout.fillWidth: true
                enabled: !dialog.backend.signingIn
                textRole: "text"
                valueRole: "value"
                model: dialog.browsers
            }
            TextButton {
                theme: dialog.theme
                text: dialog.theme.tr("Take sign-in")
                enabled: !dialog.backend.signingIn
                onClicked: dialog.backend.signInBrowser(browser.currentValue)
            }
        }
        Label {
            Layout.fillWidth: true
            visible: dialog.browsers.length === 0
            text: dialog.theme.tr("No browser to take the sign-in from: Chrome, Chromium, Brave, Vivaldi and Firefox are not found here.")
            color: dialog.theme.faint
            font.pixelSize: 12
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: dialog.browsers.length > 0
            text: dialog.theme.tr("The browser must have music.youtube.com open and signed in. The sign-in follows the browser: when the browser renews its own, orbis reads it again.")
            color: dialog.theme.faint
            font.pixelSize: 12
            wrapMode: Text.Wrap
        }

        RowLayout {
            Layout.fillWidth: true
            Layout.topMargin: 8
            visible: dialog.backend.signingIn
            spacing: 8

            BusyIndicator {
                implicitWidth: 24
                implicitHeight: 24
                running: dialog.backend.signingIn
            }
            Label {
                Layout.fillWidth: true
                text: dialog.backend.signInNote
                color: dialog.theme.text
                wrapMode: Text.Wrap
            }
            TextButton {
                theme: dialog.theme
                text: dialog.theme.tr("Cancel")
                onClicked: dialog.backend.cancelSignIn()
            }
        }
        Label {
            Layout.fillWidth: true
            Layout.topMargin: 4
            visible: text.length > 0
            text: dialog.backend.signInError
            color: dialog.theme.danger
            wrapMode: Text.Wrap
        }
        TextButton {
            Layout.alignment: Qt.AlignRight
            Layout.topMargin: 4
            visible: !dialog.backend.signingIn
            theme: dialog.theme
            text: dialog.theme.tr("Close")
            flat: true
            onClicked: dialog.close()
        }
    }
}
