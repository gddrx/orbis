import QtQuick

// A cover from the core's cache: asked for at the size it is shown (device
// pixels), decoded to that size, drawn once its file is there. Only delegates
// that exist ask, so a scrolled page fetches what comes into view.
Rectangle {
    id: cover

    required property var covers
    property string url: ""
    // The local file, or empty while it downloads.
    property string local: ""
    readonly property int pixelWidth: Math.ceil(width * Screen.devicePixelRatio)
    readonly property int pixelHeight: Math.ceil(height * Screen.devicePixelRatio)

    function fetch() {
        local = url.length > 0 && pixelWidth > 0 && pixelHeight > 0
            ? covers.request(url, pixelWidth, pixelHeight) : ""
    }

    color: "#24242e"
    onUrlChanged: fetch()
    onPixelWidthChanged: fetch()
    onPixelHeightChanged: fetch()
    Component.onCompleted: fetch()

    Image {
        anchors.fill: parent
        visible: status === Image.Ready
        source: cover.local
        sourceSize.width: cover.pixelWidth
        sourceSize.height: cover.pixelHeight
        asynchronous: true
        fillMode: Image.PreserveAspectCrop
    }

    Connections {
        target: cover.covers
        enabled: cover.local.length === 0 && cover.url.length > 0
        function onCoverReady(url) {
            if (url === cover.url)
                cover.fetch()
        }
    }
}
