import QtQuick
import QtQuick.Controls.Basic

// The menu of a track, one for the whole window. The caller first points the
// backend at the track (menuForRow, menuForCard, menuForCurrent), then calls
// popup(); the album, artists and rating come a moment later, so the items
// that need them wait for menuReady.
Menu {
    // Not `menu`: inside a MenuItem that name is the item's own property
    // MenuItem.menu, which turns null when the Instantiator below takes an
    // artist's item out, and its text binding then fails.
    id: trackMenu

    required property var backend
    required property var playlistDialog
    required property var theme

    // Where the artists' items go: after "Album".
    readonly property int artistsAt: 5

    width: 300

    MenuItem {
        text: trackMenu.theme.tr("Play next")
        onTriggered: trackMenu.backend.menuPlayNext()
    }
    MenuItem {
        text: trackMenu.theme.tr("Add to queue")
        onTriggered: trackMenu.backend.menuAddToQueue()
    }
    MenuItem {
        text: trackMenu.theme.tr("Start radio")
        onTriggered: trackMenu.backend.menuRadio()
    }
    MenuSeparator {}
    MenuItem {
        text: trackMenu.backend.menuAlbum.length > 0 ? trackMenu.theme.tr("Album: %1").arg(trackMenu.backend.menuAlbum)
            : (trackMenu.backend.menuReady ? trackMenu.theme.tr("No album") : trackMenu.theme.tr("Album…"))
        enabled: trackMenu.backend.menuAlbum.length > 0
        onTriggered: trackMenu.backend.menuOpenAlbum()
    }

    // One item per artist, inserted after the album.
    Instantiator {
        model: trackMenu.backend.menuArtists
        delegate: MenuItem {
            required property int index
            required property string modelData
            text: trackMenu.theme.tr("Artist: %1").arg(modelData)
            onTriggered: trackMenu.backend.menuOpenArtist(index)
        }
        onObjectAdded: (index, object) => trackMenu.insertItem(trackMenu.artistsAt + index, object)
        onObjectRemoved: (index, object) => trackMenu.removeItem(object)
    }

    MenuSeparator {}
    MenuItem {
        text: trackMenu.backend.menuRating === "like" ? trackMenu.theme.tr("Remove from liked") : trackMenu.theme.tr("Like")
        enabled: trackMenu.backend.menuReady
        onTriggered: trackMenu.backend.menuLike()
    }
    MenuItem {
        text: trackMenu.backend.menuRating === "dislike" ? trackMenu.theme.tr("Remove dislike") : trackMenu.theme.tr("Dislike")
        enabled: trackMenu.backend.menuReady
        onTriggered: trackMenu.backend.menuDislike()
    }
    MenuItem {
        text: trackMenu.theme.tr("Add to playlist…")
        onTriggered: trackMenu.playlistDialog.open()
    }
    MenuItem {
        text: trackMenu.theme.tr("Download")
        onTriggered: trackMenu.backend.menuDownload()
    }
}
