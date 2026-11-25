import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Dialogs
import QtQuick.Layouts

// The settings: each change applies at once and is saved to
// ~/.config/orbis/config.toml. Interface: the language, the glass and the
// tray. Account: who is signed in, how, signing out.
Popup {
    id: dialog

    required property var settings
    required property var backend
    required property var theme

    // The account section asks for a new sign-in: the window's sign-in
    // dialog opens.
    signal signInRequested()

    anchors.centerIn: Overlay.overlay
    width: Math.min(600, Overlay.overlay ? Overlay.overlay.width - 40 : 600)
    height: Math.min(560, Overlay.overlay ? Overlay.overlay.height - 40 : 560)
    modal: true
    focus: true
    padding: 0
    closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside
    background: Rectangle {
        color: dialog.theme.raised
        radius: 10
    }

    Connections {
        target: dialog.settings
        function onTabRequested(index) { tabs.currentIndex = index }
        // A template changed elsewhere (the reset, a second window) shows
        // in the field, unless it is being typed.
        function onChanged() {
            if (!template.activeFocus && template.text !== dialog.settings.template)
                template.text = dialog.settings.template
        }
    }

    onOpened: {
        signOutConfirm.visible = false
        template.text = dialog.settings.template
        dialog.backend.loadAccount()
    }

    // A line of help under a control.
    component Hint: Label {
        Layout.fillWidth: true
        color: dialog.theme.faint
        font.pixelSize: 12
        wrapMode: Text.Wrap
    }
    // A switch that shows its state on the dark ground: the accent when on.
    component Toggle: Switch {
        palette.dark: dialog.theme.accent
        palette.midlight: dialog.theme.lifted
        palette.window: dialog.theme.text
        palette.light: dialog.theme.dim
    }
    // A section's tab: a pill, filled when chosen.
    component Section: TabButton {
        id: section

        implicitHeight: 36
        contentItem: Text {
            text: section.text
            font: section.font
            color: section.checked ? dialog.theme.text : dialog.theme.dim
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
        }
        background: Rectangle {
            radius: dialog.theme.radius
            color: section.checked ? dialog.theme.selected
                : (section.hovered ? dialog.theme.hover : "transparent")
        }
    }
    component Heading: Label {
        Layout.topMargin: 12
        color: dialog.theme.text
        font.bold: true
    }

    contentItem: ColumnLayout {
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.margins: 16
            Layout.bottomMargin: 8

            Label {
                Layout.fillWidth: true
                text: dialog.theme.tr("Settings")
                color: dialog.theme.text
                font.pixelSize: 18
                font.bold: true
            }
            Label {
                text: "orbis " + dialog.settings.version
                color: dialog.theme.faint
                font.pixelSize: 12
            }
            IconButton {
                theme: dialog.theme
                iconName: "close"
                tip: dialog.theme.tr("Close")
                onClicked: dialog.close()
            }
        }

        TabBar {
            id: tabs

            Layout.fillWidth: true
            Layout.leftMargin: 16
            Layout.rightMargin: 16
            spacing: 4
            background: Item {}

            Section { text: dialog.theme.tr("Playback") }
            Section { text: dialog.theme.tr("Downloads") }
            Section { text: dialog.theme.tr("Interface") }
            Section { text: dialog.theme.tr("Account") }
        }

        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.margins: 16
            currentIndex: tabs.currentIndex

            // ---- playback ----------------------------------------------------
            ColumnLayout {
                spacing: 6

                Label { text: dialog.theme.tr("Quality"); color: dialog.theme.text }
                Choice {
                    Layout.fillWidth: true
                    theme: dialog.theme
                    textRole: "text"
                    valueRole: "value"
                    model: [
                        { text: dialog.theme.tr("Best — Opus ~256 kbit/s (Premium)"), value: "best" },
                        { text: dialog.theme.tr("Normal — Opus ~130–160 kbit/s"), value: "normal" },
                        { text: dialog.theme.tr("Economy — Opus ~50–70 kbit/s"), value: "economy" }
                    ]
                    currentIndex: model.findIndex(m => m.value === dialog.settings.quality)
                    onActivated: dialog.settings.setQuality(currentValue)
                }
                Hint { text: dialog.theme.tr("From the next track on. Downloads always take the best stream.") }

                Toggle {
                    Layout.topMargin: 12
                    text: dialog.theme.tr("Even out loudness")
                    checked: dialog.settings.normalize
                    onToggled: dialog.settings.setNormalize(checked)
                }
                Hint { text: dialog.theme.tr("Tracks that are too loud play quieter, down to YouTube's level (−14 LUFS). Quiet ones are not raised.") }

                Toggle {
                    Layout.topMargin: 12
                    text: dialog.theme.tr("Skip tracks I disliked")
                    checked: dialog.settings.skipDisliked
                    onToggled: dialog.settings.setSkipDisliked(checked)
                }
                Hint { text: dialog.theme.tr("Next and the end of a track go past them. A disliked track you click still plays.") }

                Toggle {
                    text: dialog.theme.tr("Hide disliked tracks in lists")
                    checked: dialog.settings.hideDisliked
                    onToggled: dialog.settings.setHideDisliked(checked)
                }
                Hint { text: dialog.theme.tr("Playlists, mixes and radios load without them. Lists already shown change when loaded again.") }

                Item { Layout.fillHeight: true }
            }

            // ---- downloads ---------------------------------------------------
            Flickable {
                clip: true
                contentWidth: width
                contentHeight: downloads.implicitHeight
                boundsBehavior: Flickable.StopAtBounds
                ScrollBar.vertical: ScrollBar {}

                ColumnLayout {
                    id: downloads

                    width: parent.width - 12
                    spacing: 6

                    Label { text: dialog.theme.tr("Format"); color: dialog.theme.text }
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8

                        Choice {
                            Layout.fillWidth: true
                            theme: dialog.theme
                            textRole: "text"
                            valueRole: "value"
                            model: [
                                { text: dialog.theme.tr("Opus — as streamed, no re-encoding"), value: "opus" },
                                { text: "MP3", value: "mp3" },
                                { text: "M4A (AAC)", value: "m4a" }
                            ]
                            currentIndex: model.findIndex(m => m.value === dialog.settings.downloadFormat)
                            onActivated: dialog.settings.setDownloadFormat(currentValue)
                        }
                        Choice {
                            visible: dialog.settings.downloadFormat === "mp3"
                            theme: dialog.theme
                            model: [192, 256, 320]
                            displayText: dialog.theme.tr("%1 kbit/s").arg(currentValue)
                            currentIndex: model.indexOf(dialog.settings.mp3Bitrate)
                            onActivated: dialog.settings.setMp3Bitrate(currentValue)
                        }
                        Choice {
                            visible: dialog.settings.downloadFormat === "m4a"
                            theme: dialog.theme
                            model: [128, 192, 256]
                            displayText: dialog.theme.tr("%1 kbit/s").arg(currentValue)
                            currentIndex: model.indexOf(dialog.settings.m4aBitrate)
                            onActivated: dialog.settings.setM4aBitrate(currentValue)
                        }
                    }

                    Heading { text: dialog.theme.tr("Folder") }
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8

                        Label {
                            Layout.fillWidth: true
                            text: dialog.settings.downloadFolder
                            color: dialog.theme.dim
                            elide: Text.ElideMiddle
                        }
                        TextButton {
                            theme: dialog.theme
                            text: dialog.theme.tr("Choose…")
                            onClicked: folderDialog.open()
                        }
                        TextButton {
                            visible: !dialog.settings.folderIsDefault
                            theme: dialog.theme
                            text: dialog.theme.tr("System default")
                            onClicked: dialog.settings.resetDownloadFolder()
                        }
                    }

                    Heading { text: dialog.theme.tr("File name") }
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8

                        Field {
                            id: template

                            Layout.fillWidth: true
                            theme: dialog.theme
                            onTextEdited: dialog.settings.setTemplate(text)
                        }
                        TextButton {
                            theme: dialog.theme
                            text: dialog.theme.tr("Default")
                            onClicked: {
                                dialog.settings.resetTemplate()
                                template.text = dialog.settings.template
                            }
                        }
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: text.length > 0
                        text: dialog.settings.templateError
                        color: dialog.theme.danger
                    }
                    Label {
                        Layout.fillWidth: true
                        text: dialog.settings.templateExample
                        color: dialog.theme.dim
                        font.family: "monospace"
                        font.pixelSize: 12
                        wrapMode: Text.Wrap
                    }
                    Hint { text: dialog.theme.tr("Fields: {album_artist} {artist} {album} {year} {track} {title}; / makes a folder. An empty field drops out with its \" - \". Tracks already downloaded are looked for by the new name: under the old one they are not recognized.") }

                    Toggle {
                        Layout.topMargin: 12
                        text: dialog.theme.tr("Refine tags with MusicBrainz")
                        checked: dialog.settings.musicbrainz
                        onToggled: dialog.settings.setMusicbrainz(checked)
                    }
                    Hint { text: dialog.theme.tr("Genre, first release date, disc number. MusicBrainz receives the track's artist, title and album.") }
                }
            }

            // ---- interface: language, window and tray -------------------------
            ColumnLayout {
                spacing: 6

                Label { text: dialog.theme.tr("Language"); color: dialog.theme.text }
                Choice {
                    Layout.preferredWidth: 220
                    theme: dialog.theme
                    textRole: "text"
                    valueRole: "value"
                    // Each language by its own name.
                    model: [
                        { text: "English", value: "en" },
                        { text: "Українська", value: "uk" }
                    ]
                    currentIndex: model.findIndex(m => m.value === dialog.settings.language)
                    onActivated: dialog.settings.setLanguage(currentValue)
                }

                Toggle {
                    Layout.topMargin: 12
                    text: dialog.theme.tr("Glass")
                    checked: dialog.settings.glassOn
                    onToggled: dialog.settings.setGlass(checked)
                }
                Hint { text: dialog.theme.tr("KWin blurs what is behind the window. Off: the window is opaque.") }

                Label {
                    Layout.topMargin: 12
                    text: dialog.theme.tr("Background opacity: %1 %").arg(dialog.settings.opacity)
                    color: dialog.settings.glassOn ? dialog.theme.text : dialog.theme.faint
                }
                Seek {
                    Layout.fillWidth: true
                    theme: dialog.theme
                    enabled: dialog.settings.glassOn
                    from: 40
                    to: 90
                    stepSize: 5
                    snapMode: Slider.SnapAlways
                    value: dialog.settings.opacity
                    onMoved: dialog.settings.setOpacity(value)
                }
                Hint { text: dialog.theme.tr("Less shows more of the desktop. Below 40 % text is hard to read over light windows.") }

                Toggle {
                    Layout.topMargin: 12
                    text: dialog.theme.tr("Icon in the tray")
                    checked: dialog.settings.tray
                    onToggled: dialog.settings.setTray(checked)
                }
                Hint { text: dialog.theme.tr("× hides the window in the tray. Without the icon × closes orbis.") }

                Toggle {
                    text: dialog.theme.tr("Unload the window in the tray")
                    enabled: dialog.settings.tray
                    checked: dialog.settings.unloadInTray
                    onToggled: dialog.settings.setUnloadInTray(checked)
                }
                Hint { text: dialog.theme.tr("The hidden window frees its memory and is built again when opened; the music plays on.") }

                Item { Layout.fillHeight: true }
            }

            // ---- account -----------------------------------------------------
            ColumnLayout {
                spacing: 6

                Label {
                    Layout.fillWidth: true
                    text: !dialog.backend.signedIn ? dialog.theme.tr("Not signed in to YouTube Music.")
                        : dialog.backend.accountName.length > 0 ? dialog.backend.accountName
                        : dialog.theme.tr("Loading the account…")
                    color: dialog.backend.signedIn ? dialog.theme.text : dialog.theme.danger
                    font.pixelSize: 15
                    wrapMode: Text.Wrap
                }
                Label {
                    visible: dialog.backend.signedIn && dialog.backend.accountSource.length > 0
                    text: dialog.backend.accountSource
                    color: dialog.theme.dim
                }

                RowLayout {
                    Layout.topMargin: 12
                    spacing: 8

                    TextButton {
                        theme: dialog.theme
                        accent: !dialog.backend.signedIn
                        text: dialog.backend.signedIn ? dialog.theme.tr("Sign in again…") : dialog.theme.tr("Sign in…")
                        onClicked: dialog.signInRequested()
                    }
                    TextButton {
                        visible: dialog.backend.signedIn && !signOutConfirm.visible
                        theme: dialog.theme
                        text: dialog.theme.tr("Sign out")
                        onClicked: signOutConfirm.visible = true
                    }
                }
                Hint { text: dialog.theme.tr("\"Sign in again\" changes the way of signing in or the account: a Google window or a browser's sign-in.") }

                ColumnLayout {
                    id: signOutConfirm

                    Layout.fillWidth: true
                    Layout.topMargin: 12
                    visible: false
                    spacing: 6

                    Label {
                        Layout.fillWidth: true
                        text: dialog.theme.tr("Sign out of the account? orbis forgets the sign-in, music and downloads stop.")
                        color: dialog.theme.text
                        wrapMode: Text.Wrap
                    }
                    // Off, the tabs stay for the next sign-in; on, another
                    // account does not find them.
                    Toggle {
                        id: closeTabs
                        text: dialog.theme.tr("Close the tabs and queues too")
                    }
                    RowLayout {
                        spacing: 8

                        TextButton {
                            theme: dialog.theme
                            danger: true
                            text: dialog.theme.tr("Sign out")
                            onClicked: {
                                signOutConfirm.visible = false
                                dialog.backend.signOut(closeTabs.checked)
                                closeTabs.checked = false
                            }
                        }
                        TextButton {
                            theme: dialog.theme
                            text: dialog.theme.tr("Cancel")
                            flat: true
                            onClicked: signOutConfirm.visible = false
                        }
                    }
                }

                Item { Layout.fillHeight: true }
            }
        }

        Label {
            Layout.fillWidth: true
            Layout.margins: 16
            Layout.topMargin: 0
            visible: text.length > 0
            text: dialog.settings.problem
            color: dialog.theme.danger
            wrapMode: Text.Wrap
        }
    }

    FolderDialog {
        id: folderDialog

        title: dialog.theme.tr("Folder for downloads")
        currentFolder: "file://" + dialog.settings.downloadFolder
        onAccepted: dialog.settings.setDownloadFolder(
            decodeURIComponent(selectedFolder.toString().replace(/^file:\/\//, "")))
    }
}
