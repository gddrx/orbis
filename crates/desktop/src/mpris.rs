//! MPRIS: the player on the bus for media keys, Plasma's media player widget
//! and KDE Connect — `org.mpris.MediaPlayer2` and its `.Player` at
//! `/org/mpris/MediaPlayer2`. It shows what it is given ([`MprisState`]) and
//! reports what is asked of it ([`MprisEvent`]); the app does it.
//!
//! The same bus name keeps the app to one instance: [`Mpris::claim`] fails
//! over to [`Mpris::show_running`], which asks the running one to show its
//! window through `org.freedesktop.Application.Activate`, passing the XDG
//! activation token the launcher gave.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::object_server::SignalEmitter;
use zbus::{Connection, ObjectPath, OwnedObjectPath, OwnedVariant, Variant};

const PATH: &str = "/org/mpris/MediaPlayer2";
/// Where `org.freedesktop.Application` is served.
const APP_PATH: &str = "/orbis";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

#[derive(Debug, thiserror::Error)]
pub enum MprisError {
    #[error("mpris: {0}")]
    Dbus(#[from] zbus::Error),
    #[error("mpris: {0}")]
    Fdo(#[from] zbus::fdo::Error),
}

/// The track that plays or is paused.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MprisTrack {
    /// Unique per track (a video ID); it makes the track's object path.
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub length: Option<Duration>,
    /// A `file://` URL of the cover.
    pub art_url: Option<String>,
    /// Where the track lives on the web.
    pub url: Option<String>,
}

/// What MPRIS shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MprisState {
    pub track: Option<MprisTrack>,
    pub playing: bool,
    /// 0 to 1.
    pub volume: f64,
}

/// What a client asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum MprisEvent {
    /// Show the window; `token` is the XDG activation token, when the ask
    /// came from a second start of the app.
    Raise {
        token: Option<String>,
    },
    Quit,
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    /// Move by this much, back when negative.
    Seek {
        by: Duration,
        back: bool,
    },
    /// Move to this place in the track `track` (its ID).
    SetPosition {
        track: String,
        position: Duration,
    },
    /// 0 to 1.
    Volume(f64),
}

/// Where the player is in the track now; asked on every `Position` read.
type PositionSource = Box<dyn Fn() -> Option<Duration> + Send + Sync>;

struct Shared {
    identity: String,
    state: Mutex<MprisState>,
    position: Mutex<Option<PositionSource>>,
    events: UnboundedSender<MprisEvent>,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, MprisState> {
        self.state.lock().expect("mpris state lock")
    }

    fn send(&self, event: MprisEvent) {
        let _ = self.events.send(event);
    }

    fn has_track(&self) -> bool {
        self.state().track.is_some()
    }
}

/// The object path of track `id`: path elements allow only `[A-Za-z0-9_]`,
/// so the ID is written in hex.
fn track_path(id: &str) -> OwnedObjectPath {
    let hex: String = id.bytes().map(|b| format!("{b:02x}")).collect();
    OwnedObjectPath::try_from(format!("/org/orbis/track/t{hex}"))
        .expect("hex makes a valid object path")
}

fn micros(duration: Duration) -> i64 {
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}

fn owned<'a>(value: impl Into<Variant<'a>>) -> OwnedVariant {
    value
        .into()
        .try_into_owned()
        .expect("metadata holds no file descriptors")
}

fn metadata(track: Option<&MprisTrack>) -> HashMap<String, OwnedVariant> {
    let mut map = HashMap::new();
    let Some(track) = track else {
        map.insert(
            "mpris:trackid".to_owned(),
            owned(ObjectPath::from_static_str_unchecked(NO_TRACK)),
        );
        return map;
    };
    map.insert("mpris:trackid".to_owned(), owned(track_path(&track.id)));
    map.insert("xesam:title".to_owned(), owned(track.title.as_str()));
    map.insert("xesam:artist".to_owned(), owned(track.artists.clone()));
    if let Some(album) = &track.album {
        map.insert("xesam:album".to_owned(), owned(album.as_str()));
    }
    if let Some(length) = track.length {
        map.insert("mpris:length".to_owned(), owned(micros(length)));
    }
    if let Some(art) = &track.art_url {
        map.insert("mpris:artUrl".to_owned(), owned(art.as_str()));
    }
    if let Some(url) = &track.url {
        map.insert("xesam:url".to_owned(), owned(url.as_str()));
    }
    map
}

/// `org.mpris.MediaPlayer2`.
struct Root(Arc<Shared>);

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        self.0.send(MprisEvent::Raise { token: None });
    }

    fn quit(&self) {
        self.0.send(MprisEvent::Quit);
    }

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        self.0.identity.clone()
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        self.0.identity.clone()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

/// `org.mpris.MediaPlayer2.Player`.
struct Player(Arc<Shared>);

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) {
        self.0.send(MprisEvent::Next);
    }

    fn previous(&self) {
        self.0.send(MprisEvent::Previous);
    }

    fn pause(&self) {
        self.0.send(MprisEvent::Pause);
    }

    fn play_pause(&self) {
        self.0.send(MprisEvent::PlayPause);
    }

    fn stop(&self) {
        self.0.send(MprisEvent::Stop);
    }

    fn play(&self) {
        self.0.send(MprisEvent::Play);
    }

    fn seek(&self, offset: i64) {
        self.0.send(MprisEvent::Seek {
            by: Duration::from_micros(offset.unsigned_abs()),
            back: offset < 0,
        });
    }

    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        let current = self.0.state().track.as_ref().map(|t| track_path(&t.id));
        // Positions for another track, or outside it, are ignored (MPRIS).
        if let (Some(current), Ok(position)) = (current, u64::try_from(position))
            && current.as_str() == track_id.as_str()
            && let Some(track) = self.0.state().track.as_ref()
        {
            self.0.send(MprisEvent::SetPosition {
                track: track.id.clone(),
                position: Duration::from_micros(position),
            });
        }
    }

    fn open_uri(&self, _uri: String) -> zbus::fdo::Result<()> {
        Err(zbus::fdo::Error::NotSupported(
            "orbis opens no URIs".to_owned(),
        ))
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> &str {
        let state = self.0.state();
        match (&state.track, state.playing) {
            (None, _) => "Stopped",
            (Some(_), true) => "Playing",
            (Some(_), false) => "Paused",
        }
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn set_rate(&self, _rate: f64) {}

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedVariant> {
        metadata(self.0.state().track.as_ref())
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.0.state().volume
    }

    #[zbus(property)]
    fn set_volume(&self, volume: f64) {
        self.0.send(MprisEvent::Volume(volume.clamp(0.0, 1.0)));
    }

    /// Asked of the player itself, so it is right even with the window
    /// unloaded; MPRIS does not signal its changes.
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        let position = self.0.position.lock().expect("mpris position lock");
        position
            .as_ref()
            .and_then(|source| source())
            .map_or(0, micros)
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.0.has_track()
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.0.has_track()
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.0.has_track()
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.0.has_track()
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.0.has_track()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }
}

/// `org.freedesktop.Application` at [`APP_PATH`]: a second start of the app
/// asks the first one to show its window.
struct Application(Arc<Shared>);

#[zbus::interface(name = "org.freedesktop.Application")]
impl Application {
    fn activate(&self, platform_data: HashMap<String, OwnedVariant>) {
        let token = ["activation-token", "desktop-startup-id"]
            .iter()
            .find_map(|key| platform_data.get(*key))
            .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
            .filter(|token| !token.is_empty());
        self.0.send(MprisEvent::Raise { token });
    }
}

/// The player on the bus.
pub struct Mpris {
    connection: Connection,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Mpris {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mpris")
            .field("identity", &self.shared.identity)
            .finish_non_exhaustive()
    }
}

fn bus_name(identity: &str) -> String {
    format!("org.mpris.MediaPlayer2.{identity}")
}

impl Mpris {
    /// Serves the player `identity` on `connection` and takes its bus name.
    /// `None` when another instance holds the name; what clients ask goes
    /// to `events`.
    pub async fn claim(
        connection: &Connection,
        identity: &str,
        events: UnboundedSender<MprisEvent>,
    ) -> Result<Option<Self>, MprisError> {
        let shared = Arc::new(Shared {
            identity: identity.to_owned(),
            state: Mutex::new(MprisState {
                volume: 1.0,
                ..MprisState::default()
            }),
            position: Mutex::new(None),
            events,
        });
        // Interfaces first: clients read them as soon as the name appears.
        let server = connection.object_server();
        server.at(PATH, Root(Arc::clone(&shared))).await?;
        server.at(PATH, Player(Arc::clone(&shared))).await?;
        server
            .at(APP_PATH, Application(Arc::clone(&shared)))
            .await?;
        let reply = connection
            .request_name_with_flags(bus_name(identity).as_str(), RequestNameFlags::DO_NOT_QUEUE)
            .await;
        let taken = match reply {
            Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => false,
            Ok(_) | Err(zbus::Error::NameTaken) => true,
            Err(e) => return Err(e.into()),
        };
        if taken {
            server.remove::<Root, _>(PATH).await?;
            server.remove::<Player, _>(PATH).await?;
            server.remove::<Application, _>(APP_PATH).await?;
            return Ok(None);
        }
        Ok(Some(Self {
            connection: connection.clone(),
            shared,
        }))
    }

    /// Asks the running instance `identity` to show its window, handing it
    /// `token`, the XDG activation token this start was given.
    pub async fn show_running(
        connection: &Connection,
        identity: &str,
        token: Option<String>,
    ) -> Result<(), MprisError> {
        let mut platform_data: HashMap<&str, Variant<'_>> = HashMap::new();
        if let Some(token) = token {
            platform_data.insert("activation-token", Variant::from(token));
        }
        connection
            .call_method(
                Some(bus_name(identity).as_str()),
                APP_PATH,
                Some("org.freedesktop.Application"),
                "Activate",
                &(platform_data,),
            )
            .await?;
        Ok(())
    }

    /// `Position` reads `source` from now on.
    pub fn set_position_source(
        &self,
        source: impl Fn() -> Option<Duration> + Send + Sync + 'static,
    ) {
        *self.shared.position.lock().expect("mpris position lock") = Some(Box::new(source));
    }

    /// Shows `state`; clients hear of every property that changed.
    pub async fn update(&self, state: MprisState) -> Result<(), MprisError> {
        let old = std::mem::replace(&mut *self.shared.state(), state.clone());
        if old == state {
            return Ok(());
        }
        let server = self.connection.object_server();
        let player = server.interface::<_, Player>(PATH).await?;
        let emitter = player.signal_emitter();
        let player = player.get().await;
        if old.track != state.track {
            player.metadata_changed(emitter).await?;
        }
        if old.track.is_some() != state.track.is_some() {
            player.can_go_next_changed(emitter).await?;
            player.can_go_previous_changed(emitter).await?;
            player.can_play_changed(emitter).await?;
            player.can_pause_changed(emitter).await?;
            player.can_seek_changed(emitter).await?;
        }
        if (old.track.is_some(), old.playing) != (state.track.is_some(), state.playing) {
            player.playback_status_changed(emitter).await?;
        }
        if old.volume != state.volume {
            player.volume_changed(emitter).await?;
        }
        Ok(())
    }

    /// Tells clients the position jumped (a seek), so they stop guessing it
    /// from the time that passed.
    pub async fn seeked(&self, position: Duration) -> Result<(), MprisError> {
        let player = self
            .connection
            .object_server()
            .interface::<_, Player>(PATH)
            .await?;
        Player::seeked(player.signal_emitter(), micros(position)).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures_util::StreamExt;
    use tokio::sync::mpsc;

    use crate::test_bus::Bus;

    const MPRIS: &str = "org.mpris.MediaPlayer2.orbis";

    fn track() -> MprisTrack {
        MprisTrack {
            id: "abc-DEF_12".into(),
            title: "Song".into(),
            artists: vec!["One".into(), "Two".into()],
            album: Some("Album".into()),
            length: Some(Duration::from_secs(200)),
            art_url: Some("file:///covers/a.jpg".into()),
            url: Some("https://music.youtube.com/watch?v=abc-DEF_12".into()),
        }
    }

    /// A client's view of the player; uncached, so reads see the latest.
    async fn player(connection: &Connection) -> zbus::Proxy<'static> {
        zbus::proxy::Builder::new(connection)
            .destination(MPRIS)
            .path(PATH)
            .interface("org.mpris.MediaPlayer2.Player")
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_player_shows_the_track_and_takes_commands() {
        let Some(bus) = Bus::start() else {
            eprintln!("no dbus-daemon: skipped");
            return;
        };
        let (sender, mut events) = mpsc::unbounded_channel();
        let connection = bus.connect().await;
        let mpris = Mpris::claim(&connection, "orbis", sender)
            .await
            .unwrap()
            .expect("the name is free");
        mpris.set_position_source(|| Some(Duration::from_secs(42)));

        let client = bus.connect().await;
        let player = player(&client).await;
        let status: String = player.get_property("PlaybackStatus").await.unwrap();
        assert_eq!(status, "Stopped");
        let can_next: bool = player.get_property("CanGoNext").await.unwrap();
        assert!(!can_next);

        // Properties-changed signals follow an update.
        let mut changes = zbus::fdo::PropertiesProxy::builder(&client)
            .destination(MPRIS)
            .path(PATH)
            .build()
            .await
            .unwrap()
            .receive_properties_changed()
            .await
            .unwrap();
        mpris
            .update(MprisState {
                track: Some(track()),
                playing: true,
                volume: 0.5,
            })
            .await
            .unwrap();
        let mut changed = Vec::new();
        while changed.len() < 8 {
            let signal = tokio::time::timeout(Duration::from_secs(2), changes.next())
                .await
                .unwrap()
                .unwrap();
            let args = signal.args().unwrap();
            changed.extend(args.changed_properties().keys().map(|k| k.to_string()));
        }
        for name in [
            "Metadata",
            "PlaybackStatus",
            "Volume",
            "CanGoNext",
            "CanSeek",
        ] {
            assert!(
                changed.iter().any(|c| c == name),
                "{name} not in {changed:?}"
            );
        }

        let metadata: HashMap<String, OwnedVariant> =
            player.get_property("Metadata").await.unwrap();
        let title = String::try_from(metadata["xesam:title"].try_clone().unwrap()).unwrap();
        assert_eq!(title, "Song");
        let artists =
            Vec::<String>::try_from(metadata["xesam:artist"].try_clone().unwrap()).unwrap();
        assert_eq!(artists, ["One", "Two"]);
        let length = i64::try_from(metadata["mpris:length"].try_clone().unwrap()).unwrap();
        assert_eq!(length, 200_000_000);
        let track_id =
            OwnedObjectPath::try_from(metadata["mpris:trackid"].try_clone().unwrap()).unwrap();
        assert_eq!(track_id, track_path("abc-DEF_12"));
        let status: String = player.get_property("PlaybackStatus").await.unwrap();
        assert_eq!(status, "Playing");
        let position: i64 = player.get_property("Position").await.unwrap();
        assert_eq!(position, 42_000_000);

        // Commands become events.
        let _: () = player.call("Next", &()).await.unwrap();
        assert_eq!(events.recv().await, Some(MprisEvent::Next));
        let _: () = player.call("PlayPause", &()).await.unwrap();
        assert_eq!(events.recv().await, Some(MprisEvent::PlayPause));
        let _: () = player.call("Seek", &(-5_000_000i64,)).await.unwrap();
        assert_eq!(
            events.recv().await,
            Some(MprisEvent::Seek {
                by: Duration::from_secs(5),
                back: true
            })
        );
        // A position for another track is ignored; for this one it is taken.
        let other = track_path("other");
        let _: () = player
            .call("SetPosition", &(other.as_ref(), 1_000_000i64))
            .await
            .unwrap();
        let _: () = player
            .call("SetPosition", &(track_id.as_ref(), 9_000_000i64))
            .await
            .unwrap();
        assert_eq!(
            events.recv().await,
            Some(MprisEvent::SetPosition {
                track: "abc-DEF_12".into(),
                position: Duration::from_secs(9)
            })
        );
        player.set_property("Volume", 1.5f64).await.unwrap();
        assert_eq!(events.recv().await, Some(MprisEvent::Volume(1.0)));

        let root = zbus::Proxy::new(&client, MPRIS, PATH, "org.mpris.MediaPlayer2")
            .await
            .unwrap();
        let identity: String = root.get_property("Identity").await.unwrap();
        assert_eq!(identity, "orbis");
        let _: () = root.call("Raise", &()).await.unwrap();
        assert_eq!(events.recv().await, Some(MprisEvent::Raise { token: None }));

        // A seek is announced.
        let mut seeks = player.receive_signal("Seeked").await.unwrap();
        mpris.seeked(Duration::from_secs(30)).await.unwrap();
        let seeked = seeks.next().await.unwrap();
        let position: i64 = seeked.body().deserialize().unwrap();
        assert_eq!(position, 30_000_000);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_second_start_shows_the_first_one() {
        let Some(bus) = Bus::start() else {
            eprintln!("no dbus-daemon: skipped");
            return;
        };
        let (sender, mut events) = mpsc::unbounded_channel();
        let first = bus.connect().await;
        let _mpris = Mpris::claim(&first, "orbis", sender)
            .await
            .unwrap()
            .unwrap();

        let (second_sender, _second_events) = mpsc::unbounded_channel();
        let second = bus.connect().await;
        assert!(
            Mpris::claim(&second, "orbis", second_sender)
                .await
                .unwrap()
                .is_none()
        );
        Mpris::show_running(&second, "orbis", Some("token-7".into()))
            .await
            .unwrap();
        assert_eq!(
            events.recv().await,
            Some(MprisEvent::Raise {
                token: Some("token-7".into())
            })
        );
        Mpris::show_running(&second, "orbis", None).await.unwrap();
        assert_eq!(events.recv().await, Some(MprisEvent::Raise { token: None }));
    }
}
