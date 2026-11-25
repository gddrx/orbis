// What the window asks of the compositor through KF6 WindowSystem: the blur
// behind it (KWindowEffects) and focus with an activation token
// (KWindowSystem); and the window's icon. Qt Bridge gives Rust no handle of
// the window, so these few functions are C++; Rust calls them from the GUI
// thread.
//
// On Plasma 6.8 KWin blurs only through the ext-background-effect-v1 Wayland
// protocol, older KWin through org_kde_kwin_blur; KWindowEffects speaks both.

#include <KWindowEffects>
#include <KWindowSystem>
#include <QGuiApplication>
#include <QIcon>
#include <QImage>
#include <QPixmap>
#include <QWindow>

#include <cstddef>

extern "C" {

// Asks the compositor to blur behind every top-level window of the app, or
// to stop. KWindowEffects keeps the wish and applies it again whenever a
// window's surface is created anew (after it was hidden, for one).
void orbis_blur_behind(bool enable)
{
    const auto windows = QGuiApplication::topLevelWindows();
    for (QWindow *window : windows) {
        if (window->type() == Qt::Window)
            KWindowEffects::enableBlurBehind(window, enable);
    }
}

// Whether the compositor blurs behind windows. The first answer after start
// is false: the compositor's interfaces are bound a round trip later.
bool orbis_blur_available()
{
    return KWindowEffects::isEffectAvailable(KWindowEffects::BlurBehind);
}

// Gives the app's windows the orbis icon, from a PNG. On Wayland the
// compositor finds a window's icon by its app id among the installed menu
// entries; an AppImage that is not in the menu has none there. Qt 6.9 and
// later also hand the icon itself to the compositor (the xdg-toplevel-icon
// protocol), so the title bar and the task manager show it either way.
void orbis_set_window_icon(const unsigned char *png, size_t size)
{
    QImage image;
    if (!image.loadFromData(png, int(size), "PNG"))
        return;
    QIcon icon;
    for (int px : {32, 48, 64, 96, 128, 256})
        icon.addPixmap(QPixmap::fromImage(
            image.scaled(px, px, Qt::KeepAspectRatio, Qt::SmoothTransformation)));
    QGuiApplication::setWindowIcon(icon);
}

// Raises and focuses the app's shown window. On Wayland the compositor
// gives focus only with an XDG activation token, such as the one the tray
// hands with a click; `token` may be empty or null without one.
void orbis_activate_window(const char *token)
{
    const auto windows = QGuiApplication::topLevelWindows();
    for (QWindow *window : windows) {
        if (window->type() != Qt::Window || !window->isVisible())
            continue;
        if (token && *token)
            KWindowSystem::setCurrentXdgActivationToken(QString::fromUtf8(token));
        KWindowSystem::activateWindow(window);
        return;
    }
}

}
