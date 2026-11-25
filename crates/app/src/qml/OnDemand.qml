import QtQuick

// A popup made on its first use, so that the window's start builds only
// what is on screen. It answers the popup's own calls — open(), popup(),
// close() and `opened` — so the rest of the window uses it as the popup.
Loader {
    id: demand

    readonly property bool opened: item !== null && item.opened
    // Visible already while its opening transition runs (smoke test).
    readonly property bool shown: item !== null && item.visible

    function open() {
        demand.active = true
        demand.item.open()
    }
    function popup() {
        demand.active = true
        demand.item.popup()
    }
    function close() {
        if (demand.item)
            demand.item.close()
    }

    active: false
}
