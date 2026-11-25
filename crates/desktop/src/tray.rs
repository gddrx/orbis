//! The tray icon: a StatusNotifierItem — the protocol Plasma's system tray
//! (and GNOME's AppIndicator extension) shows — with a `com.canonical.dbusmenu`
//! menu, on the session bus.
//!
//! The icon only shows what it is given ([`TrayState`]) and reports what the
//! user did ([`TrayEvent`]); the app decides what that means. It registers with
//! the `org.kde.StatusNotifierWatcher` and again whenever the watcher comes back
//! (Plasma's shell restarted); [`Tray::shown`] tells whether a tray has it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;
use zbus::Connection;
use zbus::object_server::SignalEmitter;
use zbus::proxy::CacheProperties;
use zbus::wire::StructureBuilder;
use zbus::{ObjectPath, OwnedVariant, Variant};

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
/// Plasma hands the click's activation token just before the click itself;
/// an older token belongs to something else.
const TOKEN_MAX_AGE: Duration = Duration::from_secs(1);

#[derive(Debug, thiserror::Error)]
pub enum TrayError {
    #[error("tray: {0}")]
    Dbus(#[from] zbus::Error),
}

/// One line of the tray's menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Item {
        /// Comes back in [`TrayEvent::Menu`].
        action: u32,
        label: String,
        enabled: bool,
        /// `Some` makes a check mark item.
        checked: Option<bool>,
    },
    Separator,
}

/// Pictures of an icon: width, height and ARGB32 pixels in network byte
/// order, as StatusNotifierItem's `IconPixmap` carries them.
pub type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

/// What the tray shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrayState {
    /// An icon name from the icon theme, or empty for `pixmaps`.
    pub icon: String,
    /// The icon's pictures in several sizes: the tray takes them when
    /// `icon` is empty (an AppImage's icon is not in the theme).
    pub pixmaps: std::sync::Arc<Pixmaps>,
    /// The tooltip's title and text.
    pub title: String,
    pub text: String,
    pub menu: Vec<MenuEntry>,
}

/// What the user did. `token` is the XDG activation token the tray handed
/// with the click, for raising a window on Wayland.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayEvent {
    /// A (left) click on the icon.
    Activate { token: Option<String> },
    /// A middle click on the icon.
    SecondaryActivate { token: Option<String> },
    /// The wheel over the icon, vertically; 120 is one notch, positive is up.
    Scroll { delta: i32 },
    /// A menu item was chosen.
    Menu { action: u32, token: Option<String> },
}

struct Inner {
    state: TrayState,
    /// Bumped whenever the menu changes.
    revision: u32,
    token: Option<(String, Instant)>,
}

struct Shared {
    id: String,
    inner: Mutex<Inner>,
    events: UnboundedSender<TrayEvent>,
}

impl Shared {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().expect("tray state lock")
    }

    /// The token handed just before, if it is fresh.
    fn take_token(&self) -> Option<String> {
        self.inner()
            .token
            .take()
            .filter(|(_, at)| at.elapsed() <= TOKEN_MAX_AGE)
            .map(|(token, _)| token)
    }

    fn send(&self, event: TrayEvent) {
        let _ = self.events.send(event);
    }
}

/// A menu item's properties, `a{sv}`.
type Properties = HashMap<String, OwnedVariant>;
/// A menu item with its children, `(ia{sv}av)`.
type Layout = (i32, Properties, Vec<OwnedVariant>);

/// `org.kde.StatusNotifierItem` at [`ITEM_PATH`].
struct Item(Arc<Shared>);

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    /// The menu is shown by the tray from [`MENU_PATH`].
    fn context_menu(&self, _x: i32, _y: i32) {}

    fn activate(&self, _x: i32, _y: i32) {
        let token = self.0.take_token();
        self.0.send(TrayEvent::Activate { token });
    }

    fn secondary_activate(&self, _x: i32, _y: i32) {
        let token = self.0.take_token();
        self.0.send(TrayEvent::SecondaryActivate { token });
    }

    fn scroll(&self, delta: i32, orientation: String) {
        if orientation.eq_ignore_ascii_case("vertical") && delta != 0 {
            self.0.send(TrayEvent::Scroll { delta });
        }
    }

    /// KDE's extension: the token for the click that follows.
    fn provide_xdg_activation_token(&self, token: String) {
        self.0.inner().token = Some((token, Instant::now()));
    }

    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property)]
    fn id(&self) -> String {
        self.0.id.clone()
    }

    #[zbus(property)]
    fn title(&self) -> String {
        self.0.id.clone()
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property)]
    fn window_id(&self) -> i32 {
        0
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn menu(&self) -> ObjectPath<'static> {
        ObjectPath::from_static_str_unchecked(MENU_PATH)
    }

    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn icon_name(&self) -> String {
        self.0.inner().state.icon.clone()
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        self.0.inner().state.pixmaps.as_ref().clone()
    }

    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> Pixmaps {
        Vec::new()
    }

    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> Pixmaps {
        Vec::new()
    }

    #[zbus(property)]
    fn attention_movie_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        let inner = self.0.inner();
        (
            inner.state.icon.clone(),
            Vec::new(),
            inner.state.title.clone(),
            inner.state.text.clone(),
        )
    }

    #[zbus(signal)]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

fn owned<'a>(value: impl Into<Variant<'a>>) -> OwnedVariant {
    value
        .into()
        .try_into_owned()
        .expect("menu values hold no file descriptors")
}

/// The properties of menu item `id` (0 is the root), all or only `names`.
fn item_properties(state: &TrayState, id: i32, names: &[String]) -> Option<Properties> {
    let mut props = Properties::new();
    if id == 0 {
        props.insert("children-display".into(), owned("submenu"));
    } else {
        match state.menu.get(usize::try_from(id - 1).ok()?)? {
            MenuEntry::Separator => {
                props.insert("type".into(), owned("separator"));
            }
            MenuEntry::Item {
                label,
                enabled,
                checked,
                ..
            } => {
                props.insert("label".into(), owned(label.as_str()));
                props.insert("enabled".into(), owned(*enabled));
                if let Some(checked) = checked {
                    props.insert("toggle-type".into(), owned("checkmark"));
                    props.insert("toggle-state".into(), owned(i32::from(*checked)));
                }
            }
        }
        props.insert("visible".into(), owned(true));
    }
    if !names.is_empty() {
        props.retain(|name, _| names.contains(name));
    }
    Some(props)
}

/// The menu as `(ia{sv}av)` from `parent`: the root holds every item; items
/// have no submenus.
fn layout(state: &TrayState, parent: i32, depth: i32, names: &[String]) -> Option<Layout> {
    let props = item_properties(state, parent, names)?;
    let mut children = Vec::new();
    if parent == 0 && depth != 0 {
        for id in 1..=state.menu.len() as i32 {
            let child_props = item_properties(state, id, names)?;
            let child = StructureBuilder::new()
                .add_field(id)
                .add_field(child_props)
                .add_field(Vec::<OwnedVariant>::new())
                .build()
                .expect("a menu item is a valid structure");
            children.push(owned(child));
        }
    }
    Some((parent, props, children))
}

/// `com.canonical.dbusmenu` at [`MENU_PATH`].
struct Menu(Arc<Shared>);

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, Layout)> {
        let inner = self.0.inner();
        let tree = layout(&inner.state, parent_id, recursion_depth, &property_names)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no menu item {parent_id}")))?;
        Ok((inner.revision, tree))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        property_names: Vec<String>,
    ) -> Vec<(i32, Properties)> {
        let inner = self.0.inner();
        let ids = if ids.is_empty() {
            (0..=inner.state.menu.len() as i32).collect()
        } else {
            ids
        };
        ids.into_iter()
            .filter_map(|id| Some((id, item_properties(&inner.state, id, &property_names)?)))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedVariant> {
        let inner = self.0.inner();
        item_properties(&inner.state, id, std::slice::from_ref(&name))
            .and_then(|mut props| props.remove(&name))
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no property {name} of {id}")))
    }

    fn event(&self, id: i32, event_id: String, _data: OwnedVariant, _timestamp: u32) {
        self.clicked(id, &event_id);
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedVariant, u32)>) -> Vec<i32> {
        let count = self.0.inner().state.menu.len() as i32;
        let mut errors = Vec::new();
        for (id, event_id, _, _) in events {
            if (0..=count).contains(&id) {
                self.clicked(id, &event_id);
            } else {
                errors.push(id);
            }
        }
        errors
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

impl Menu {
    fn clicked(&self, id: i32, event_id: &str) {
        if event_id != "clicked" {
            return;
        }
        let action = {
            let inner = self.0.inner();
            match usize::try_from(id - 1)
                .ok()
                .and_then(|i| inner.state.menu.get(i))
            {
                Some(MenuEntry::Item {
                    action,
                    enabled: true,
                    ..
                }) => Some(*action),
                _ => None,
            }
        };
        if let Some(action) = action {
            let token = self.0.take_token();
            self.0.send(TrayEvent::Menu { action, token });
        }
    }
}

#[zbus::proxy(
    interface = "org.kde.StatusNotifierWatcher",
    default_service = "org.kde.StatusNotifierWatcher",
    default_path = "/StatusNotifierWatcher"
)]
trait Watcher {
    fn register_status_notifier_item(&self, service: &str) -> zbus::Result<()>;

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> zbus::Result<bool>;
}

/// Registers `name` with the watcher; true when a tray will show it.
async fn register(watcher: &WatcherProxy<'_>, name: &str) -> bool {
    watcher.register_status_notifier_item(name).await.is_ok()
        && watcher
            .is_status_notifier_host_registered()
            .await
            .unwrap_or(false)
}

/// The icon in the tray, until [`Tray::stop`] or the connection closes.
#[derive(Debug)]
pub struct Tray {
    connection: Connection,
    /// The bus name the item is registered under.
    name: String,
    shown: watch::Receiver<bool>,
    follow: tokio::task::JoinHandle<()>,
}

impl Tray {
    /// Puts the icon `id` into the tray, on `connection`'s bus. Must be
    /// called within a tokio runtime; what the user does goes to `events`.
    pub async fn start(
        connection: &Connection,
        id: &str,
        state: TrayState,
        events: UnboundedSender<TrayEvent>,
    ) -> Result<Self, TrayError> {
        let shared = Arc::new(Shared {
            id: id.to_owned(),
            inner: Mutex::new(Inner {
                state,
                revision: 1,
                token: None,
            }),
            events,
        });
        let server = connection.object_server();
        server.at(ITEM_PATH, Item(Arc::clone(&shared))).await?;
        server.at(MENU_PATH, Menu(shared)).await?;
        let connection = connection.clone();
        let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
        connection.request_name(name.as_str()).await?;

        let watcher = WatcherProxy::builder(&connection)
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        let dbus = zbus::fdo::DBusProxy::new(&connection).await?;
        // Listen before registering, so a watcher that starts in between is
        // not missed.
        let mut owners = dbus
            .receive_name_owner_changed_with_args(&[(0, WATCHER)])
            .await?;
        let (shown_sender, shown) = watch::channel(register(&watcher, &name).await);
        let registered = name.clone();
        let follow = tokio::spawn(async move {
            while let Some(change) = owners.next().await {
                let Ok(args) = change.args() else {
                    continue;
                };
                let shown = if args.new_owner().is_some() {
                    register(&watcher, &name).await
                } else {
                    false
                };
                shown_sender.send_replace(shown);
            }
        });
        Ok(Self {
            connection,
            name: registered,
            shown,
            follow,
        })
    }

    /// Takes the icon out of the tray: the item leaves the bus, and the
    /// tray drops it as its name goes.
    pub async fn stop(self) -> Result<(), TrayError> {
        self.follow.abort();
        let server = self.connection.object_server();
        server.remove::<Item, _>(ITEM_PATH).await?;
        server.remove::<Menu, _>(MENU_PATH).await?;
        self.connection.release_name(self.name.as_str()).await?;
        Ok(())
    }

    /// Whether a tray shows the icon now, and every change of it.
    pub fn shown(&self) -> watch::Receiver<bool> {
        self.shown.clone()
    }

    /// Shows `state`: the tray is told what changed and asks for it again.
    pub async fn update(&self, state: TrayState) -> Result<(), TrayError> {
        let server = self.connection.object_server();
        let item = server.interface::<_, Item>(ITEM_PATH).await?;
        let (icon, tooltip, menu) = {
            let shared = &item.get().await.0;
            let mut inner = shared.inner();
            let old = std::mem::replace(&mut inner.state, state);
            let icon = old.icon != inner.state.icon || old.pixmaps != inner.state.pixmaps;
            let tooltip = icon || old.title != inner.state.title || old.text != inner.state.text;
            let menu = old.menu != inner.state.menu;
            if menu {
                inner.revision += 1;
            }
            (icon, tooltip, menu.then_some(inner.revision))
        };
        if icon {
            Item::new_icon(item.signal_emitter()).await?;
        }
        if tooltip {
            Item::new_tool_tip(item.signal_emitter()).await?;
        }
        if let Some(revision) = menu {
            let menu = server.interface::<_, Menu>(MENU_PATH).await?;
            Menu::layout_updated(menu.signal_emitter(), revision, 0).await?;
        }
        Ok(())
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.follow.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::sync::mpsc;
    use zbus::connection;

    use crate::test_bus::Bus;

    /// A tray host's watcher that records the items registered.
    #[derive(Clone, Default)]
    struct FakeWatcher(Arc<Mutex<Vec<String>>>);

    #[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
    impl FakeWatcher {
        fn register_status_notifier_item(&self, service: String) {
            self.0.lock().unwrap().push(service);
        }

        #[zbus(property)]
        fn is_status_notifier_host_registered(&self) -> bool {
            true
        }
    }

    async fn start_watcher(bus: &Bus, watcher: &FakeWatcher) -> Connection {
        connection::Builder::address(bus.address.as_str())
            .serve_at("/StatusNotifierWatcher", watcher.clone())
            .name(WATCHER)
            .build()
            .await
            .unwrap()
    }

    fn state(label: &str, liked: bool) -> TrayState {
        TrayState {
            icon: "media-playback-playing".into(),
            pixmaps: std::sync::Arc::new(vec![(1, 1, vec![0xff, 0x10, 0x20, 0x30])]),
            title: "Song".into(),
            text: "Artist".into(),
            menu: vec![
                MenuEntry::Item {
                    action: 10,
                    label: label.into(),
                    enabled: true,
                    checked: None,
                },
                MenuEntry::Separator,
                MenuEntry::Item {
                    action: 20,
                    label: "Like".into(),
                    enabled: true,
                    checked: Some(liked),
                },
                MenuEntry::Item {
                    action: 30,
                    label: "Off".into(),
                    enabled: false,
                    checked: None,
                },
            ],
        }
    }

    async fn wait_for(mut condition: impl FnMut() -> bool) {
        for _ in 0..200 {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out");
    }

    async fn item_name(watcher: &FakeWatcher) -> String {
        wait_for(|| !watcher.0.lock().unwrap().is_empty()).await;
        watcher.0.lock().unwrap()[0].clone()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_icon_registers_and_answers_the_tray() {
        let Some(bus) = Bus::start() else {
            eprintln!("no dbus-daemon: skipped");
            return;
        };
        let watcher = FakeWatcher::default();
        let _watcher_connection = start_watcher(&bus, &watcher).await;
        let (sender, mut events) = mpsc::unbounded_channel();
        let connection = bus.connect().await;
        let tray = Tray::start(&connection, "orbis", state("Pause", false), sender)
            .await
            .unwrap();
        assert!(*tray.shown().borrow());
        let name = item_name(&watcher).await;
        assert!(name.starts_with("org.kde.StatusNotifierItem-"), "{name}");

        let host = bus.connect().await;
        let item = zbus::Proxy::new(
            &host,
            name.as_str(),
            ITEM_PATH,
            "org.kde.StatusNotifierItem",
        )
        .await
        .unwrap();
        let icon: String = item.get_property("IconName").await.unwrap();
        assert_eq!(icon, "media-playback-playing");
        let pixmaps: Pixmaps = item.get_property("IconPixmap").await.unwrap();
        assert_eq!(pixmaps, [(1, 1, vec![0xff, 0x10, 0x20, 0x30])]);
        let tooltip: (String, Pixmaps, String, String) =
            item.get_property("ToolTip").await.unwrap();
        assert_eq!((tooltip.2.as_str(), tooltip.3.as_str()), ("Song", "Artist"));
        let menu_path: zbus::OwnedObjectPath = item.get_property("Menu").await.unwrap();
        assert_eq!(menu_path.as_str(), MENU_PATH);

        // A click with the token handed just before it.
        let _: () = item
            .call("ProvideXdgActivationToken", &("token-1",))
            .await
            .unwrap();
        let _: () = item.call("Activate", &(0i32, 0i32)).await.unwrap();
        assert_eq!(
            events.recv().await,
            Some(TrayEvent::Activate {
                token: Some("token-1".into())
            })
        );
        let _: () = item.call("SecondaryActivate", &(0i32, 0i32)).await.unwrap();
        assert_eq!(
            events.recv().await,
            Some(TrayEvent::SecondaryActivate { token: None })
        );
        let _: () = item.call("Scroll", &(-120i32, "horizontal")).await.unwrap();
        let _: () = item.call("Scroll", &(-120i32, "vertical")).await.unwrap();
        assert_eq!(events.recv().await, Some(TrayEvent::Scroll { delta: -120 }));

        // The menu: its layout, a click, a disabled item.
        let menu = zbus::Proxy::new(&host, name.as_str(), MENU_PATH, "com.canonical.dbusmenu")
            .await
            .unwrap();
        let (revision, root): (u32, Layout) = menu
            .call("GetLayout", &(0i32, -1i32, Vec::<String>::new()))
            .await
            .unwrap();
        assert_eq!(root.0, 0);
        assert_eq!(root.2.len(), 4);
        let first: Layout = root.2[0].try_clone().unwrap().try_into().unwrap();
        assert_eq!(first.0, 1);
        assert_eq!(
            String::try_from(first.1["label"].try_clone().unwrap()).unwrap(),
            "Pause"
        );
        let separator: Layout = root.2[1].try_clone().unwrap().try_into().unwrap();
        assert_eq!(
            String::try_from(separator.1["type"].try_clone().unwrap()).unwrap(),
            "separator"
        );
        let like: Layout = root.2[2].try_clone().unwrap().try_into().unwrap();
        assert_eq!(
            i32::try_from(like.1["toggle-state"].try_clone().unwrap()).unwrap(),
            0
        );

        let _: () = menu
            .call("Event", &(4i32, "clicked", Variant::from(0i32), 0u32))
            .await
            .unwrap();
        let _: () = menu
            .call("Event", &(1i32, "hovered", Variant::from(0i32), 0u32))
            .await
            .unwrap();
        let _: () = menu
            .call("Event", &(1i32, "clicked", Variant::from(0i32), 0u32))
            .await
            .unwrap();
        assert_eq!(
            events.recv().await,
            Some(TrayEvent::Menu {
                action: 10,
                token: None
            })
        );

        // A new state bumps the revision and the layout follows.
        let layout_updates = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("com.canonical.dbusmenu")
            .member("LayoutUpdated")
            .build()
            .unwrap();
        let mut signals = zbus::MessageStream::for_match_rule(layout_updates, &host, None)
            .await
            .unwrap();
        tray.update(state("Play", true)).await.unwrap();
        let signal = signals.next().await.unwrap().unwrap();
        let (new_revision, parent): (u32, i32) = signal.body().deserialize().unwrap();
        assert_eq!((new_revision, parent), (revision + 1, 0));
        let (_, root): (u32, Layout) = menu
            .call("GetLayout", &(0i32, -1i32, vec!["label".to_owned()]))
            .await
            .unwrap();
        let first: Layout = root.2[0].try_clone().unwrap().try_into().unwrap();
        assert_eq!(
            String::try_from(first.1["label"].try_clone().unwrap()).unwrap(),
            "Play"
        );
        assert!(!first.1.contains_key("enabled"));
        let like: Layout = root.2[2].try_clone().unwrap().try_into().unwrap();
        assert!(!like.1.contains_key("toggle-state"));
        let state: OwnedVariant = menu
            .call("GetProperty", &(3i32, "toggle-state"))
            .await
            .unwrap();
        assert_eq!(i32::try_from(state).unwrap(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_icon_leaves_the_tray_on_stop() {
        let Some(bus) = Bus::start() else {
            eprintln!("no dbus-daemon: skipped");
            return;
        };
        let watcher = FakeWatcher::default();
        let _watcher_connection = start_watcher(&bus, &watcher).await;
        let (sender, _events) = mpsc::unbounded_channel();
        let connection = bus.connect().await;
        let tray = Tray::start(&connection, "orbis", state("Pause", false), sender)
            .await
            .unwrap();
        let name = item_name(&watcher).await;
        let other = bus.connect().await;
        let dbus = zbus::fdo::DBusProxy::new(&other).await.unwrap();
        let owned = |name: String| {
            let dbus = dbus.clone();
            async move {
                dbus.name_has_owner(name.as_str().try_into().unwrap())
                    .await
                    .unwrap()
            }
        };
        assert!(owned(name.clone()).await);
        tray.stop().await.unwrap();
        assert!(!owned(name).await, "the tray sees the item go");
        // The connection lives on: the icon can come back.
        let (sender, _events) = mpsc::unbounded_channel();
        assert!(
            Tray::start(&connection, "orbis", state("Pause", false), sender)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn the_icon_follows_the_watcher() {
        let Some(bus) = Bus::start() else {
            eprintln!("no dbus-daemon: skipped");
            return;
        };
        // No tray yet: the icon waits for one.
        let (sender, _events) = mpsc::unbounded_channel();
        let connection = bus.connect().await;
        let tray = Tray::start(&connection, "orbis", state("Pause", false), sender)
            .await
            .unwrap();
        let mut shown = tray.shown();
        assert!(!*shown.borrow());

        let watcher = FakeWatcher::default();
        let watcher_connection = start_watcher(&bus, &watcher).await;
        tokio::time::timeout(Duration::from_secs(2), shown.wait_for(|s| *s))
            .await
            .unwrap()
            .unwrap();
        assert!(
            item_name(&watcher)
                .await
                .starts_with("org.kde.StatusNotifierItem-")
        );

        // The tray goes away (its shell restarts) and comes back.
        drop(watcher_connection);
        tokio::time::timeout(Duration::from_secs(2), shown.wait_for(|s| !*s))
            .await
            .unwrap()
            .unwrap();
        let again = FakeWatcher::default();
        let _again_connection = start_watcher(&bus, &again).await;
        tokio::time::timeout(Duration::from_secs(2), shown.wait_for(|s| *s))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item_name(&again).await, item_name(&watcher).await);
    }
}
