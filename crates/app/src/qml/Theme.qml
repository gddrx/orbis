import QtQuick

// Colors and sizes of the glass look, and the words of the window. Bars,
// hovers and selections are translucent, so whatever lies under the window's
// content shows through: the compositor's blur of the desktop and the
// current track's cover.
QtObject {
    // The compositor blurs what is behind the window: its ground may let it
    // through. Without the blur the window is opaque.
    property bool translucent: false
    // "#rrggbb" picked from the current track's cover, or empty.
    property string coverAccent: ""
    // How much of the dark ground covers the blur (the settings).
    property real groundOpacity: 0.6
    // The chosen language's words by their English text (`settings.strings`);
    // empty for English.
    property var strings: ({})

    // `text` in the chosen language. Bindings that call it follow a change
    // of the language: they read `strings`.
    function tr(text) {
        const translated = strings[text]
        return translated === undefined ? text : translated
    }

    // Popups, controls and the window's ground without the blur.
    readonly property color background: "#141419"
    // The window's ground: dark glass over the blurred desktop.
    readonly property color base: translucent
        ? Qt.rgba(background.r, background.g, background.b, groundOpacity) : background
    // The blurred cover over the ground lets the desktop through a little.
    readonly property real backdropOpacity: translucent ? 0.75 : 1
    readonly property color surface: "#0fffffff"
    readonly property color raised: "#24242e"
    // Hover in popups and on controls, which stay opaque.
    readonly property color lifted: "#2a2a35"
    readonly property color hover: "#14ffffff"
    readonly property color pressed: "#29ffffff"
    // The ground of buttons with text and of fields.
    readonly property color control: "#1affffff"
    readonly property color controlHover: "#26ffffff"
    readonly property color selected: Qt.rgba(accent.r, accent.g, accent.b, 0.22)
    readonly property color text: "#e8e8f0"
    readonly property color dim: "#9a9aae"
    readonly property color faint: "#6c6c80"
    readonly property color accent: coverAccent.length > 0 ? coverAccent : "#a78bfa"
    readonly property color danger: "#f87171"
    // Corners of buttons, fields and tabs.
    readonly property int radius: 8
}
