// The Google sign-in window: the page and, above it, where it is. The
// window closes by itself once the user is signed in to YouTube Music.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import QtWebEngine

ApplicationWindow {
    id: window

    required property var login

    // The window's few words by their English text, in orbis's language.
    readonly property var words: ({
        uk: {
            "Sign in to Google — orbis": "Вхід у Google — orbis",
            "Loading…": "Завантаження…",
            "Unsafe page: %1": "Небезпечна сторінка: %1",
            "Cancel": "Скасувати",
            "Signed in; going back to orbis…": "Вхід виконано — повертаємося в orbis…"
        }
    })
    function tr(text) {
        const words = window.words[window.login.language]
        return words && words[text] !== undefined ? words[text] : text
    }

    // A button with rounded corners, as in the orbis window.
    component PillButton: Button {
        id: pill

        implicitHeight: 32
        leftPadding: 14
        rightPadding: 14
        contentItem: Text {
            text: pill.text
            font: pill.font
            color: "#e6e6eb"
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
        }
        background: Rectangle {
            radius: 8
            color: pill.down ? "#40ffffff" : pill.hovered ? "#2effffff" : "#1fffffff"
        }
    }

    width: 500
    height: 760
    minimumWidth: 380
    minimumHeight: 520
    visible: true
    title: window.tr("Sign in to Google — orbis")
    color: "#141419"

    header: Rectangle {
        implicitHeight: 44
        color: "#1c1c23"

        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 14
            anchors.rightMargin: 8
            spacing: 8

            // The page's host, so it is plain where the password goes.
            Label {
                Layout.fillWidth: true
                text: window.login.host === "" ? window.tr("Loading…")
                    : window.login.secure ? window.login.host
                    : window.tr("Unsafe page: %1").arg(window.login.host)
                color: window.login.secure ? "#e6e6eb" : "#f87171"
                elide: Text.ElideMiddle
            }
            BusyIndicator {
                implicitWidth: 24
                implicitHeight: 24
                running: view.loading
            }
            // When the sign-in stops short of YouTube Music, the site's own
            // "Sign in" button takes it from there.
            PillButton {
                text: "YouTube Music"
                visible: window.login.host !== "music.youtube.com"
                onClicked: view.url = "https://music.youtube.com/"
            }
            PillButton {
                text: window.tr("Cancel")
                onClicked: window.close()
            }
        }
    }

    WebEngineView {
        id: view

        anchors.fill: parent
        profile: loginProfile
        url: window.login.startUrl
        onUrlChanged: {
            const address = url.toString();
            Qt.callLater(() => window.login.visit(address));
        }
        // Google opens some pages in a new window: keep them in this one.
        onNewWindowRequested: request => view.url = request.requestedUrl
    }

    Rectangle {
        anchors.fill: parent
        visible: window.login.done
        color: window.color

        Label {
            anchors.centerIn: parent
            text: window.tr("Signed in; going back to orbis…")
            color: "#e6e6eb"
        }
    }

    // The page sets its last cookies right after it opens.
    Timer {
        interval: 1500
        running: window.login.done
        onTriggered: {
            window.login.finish();
            Qt.quit();
        }
    }
}
