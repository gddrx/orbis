import QtQuick

// The current track's blurred cover, stretched under the whole window. A new
// cover fades in over the old one once, in 300 ms, while the old one fades
// out; nothing moves otherwise.
Item {
    id: backdrop

    // The `file://` URL of the backdrop, or empty for none.
    property string source: ""
    // Which layer holds the newest picture; the other one holds the old.
    property bool firstIsNew: false
    // 0 shows the old picture, 1 the new one.
    property real mix: 1

    function startFade() {
        if (!fade.running && mix < 1)
            fade.start()
    }

    onSourceChanged: {
        fade.stop()
        // The layer that faded out last gets the new picture.
        firstIsNew = !firstIsNew
        mix = 0
        const fresh = firstIsNew ? first : second
        fresh.source = source
        // An empty source, or a picture already decoded, fades at once.
        if (source.length === 0 || fresh.status === Image.Ready || fresh.status === Image.Error)
            startFade()
    }

    Image {
        id: first

        anchors.fill: parent
        visible: opacity > 0 && status === Image.Ready
        opacity: backdrop.firstIsNew ? backdrop.mix : 1 - backdrop.mix
        fillMode: Image.PreserveAspectCrop
        asynchronous: true
        smooth: true
        onStatusChanged: if (backdrop.firstIsNew && status !== Image.Loading) backdrop.startFade()
    }
    Image {
        id: second

        anchors.fill: parent
        visible: opacity > 0 && status === Image.Ready
        opacity: backdrop.firstIsNew ? 1 - backdrop.mix : backdrop.mix
        fillMode: Image.PreserveAspectCrop
        asynchronous: true
        smooth: true
        onStatusChanged: if (!backdrop.firstIsNew && status !== Image.Loading) backdrop.startFade()
    }

    NumberAnimation {
        id: fade

        target: backdrop
        property: "mix"
        to: 1
        duration: 300
        easing.type: Easing.InOutQuad
        // The faded picture is let go.
        onFinished: (backdrop.firstIsNew ? second : first).source = ""
    }
}
