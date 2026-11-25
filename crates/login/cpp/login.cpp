// The web engine's side of the sign-in window. Qt Bridge cannot call
// QtWebEngineQuick::initialize() before it creates the application object,
// and QML gets no cookies (the profile's cookie store is C++ only, and the
// page's JavaScript does not see HttpOnly cookies), so these two functions
// are C++. Rust calls them from the main thread.

#include <cstddef>
#include <cstdint>

#include <QByteArray>
#include <QDateTime>
#include <QNetworkCookie>
#include <QQmlContext>
#include <QQmlEngine>
#include <QString>
#include <QtWebEngineCore/QWebEngineCookieStore>
#include <QtWebEngineQuick/QQuickWebEngineProfile>
#include <QtWebEngineQuick/qtwebenginequickglobal.h>

extern "C" {

// One cookie as the store reports it; the pointers live for the call only.
struct orbis_cookie {
    const char *name;
    size_t name_len;
    const char *value;
    size_t value_len;
    // ".youtube.com" for a domain cookie, "music.youtube.com" for a
    // host-only one.
    const char *domain;
    size_t domain_len;
    const char *path;
    size_t path_len;
    // Unix seconds; negative for a session cookie.
    int64_t expires;
    bool secure;
    bool http_only;
};

typedef void (*orbis_cookie_fn)(void *context, bool added, const struct orbis_cookie *cookie);

// Before the application object: shares OpenGL contexts between the web
// engine and Qt Quick.
void orbis_login_initialize()
{
    QtWebEngineQuick::initialize();
}

// Makes the window's profile and gives it to QML as `loginProfile`. The
// profile is off the record: cookies, cache and storage stay in memory and
// go with the process. Every cookie the store adds or removes is reported
// to `callback` with `context`, on the main thread.
void orbis_login_install(void *engine, orbis_cookie_fn callback, void *context)
{
    auto *qml = static_cast<QQmlEngine *>(engine);
    // Qt WebEngine 6.140 warns that profiles should come from
    // WebEngineProfilePrototype in QML; its profile is the same class, but
    // its cookie store would have to be fetched back from QML, so the
    // profile is still made here.
    auto *profile = new QQuickWebEngineProfile(qml);
    profile->setOffTheRecord(true);
    profile->setHttpCacheType(QQuickWebEngineProfile::MemoryHttpCache);
    profile->setPersistentCookiesPolicy(QQuickWebEngineProfile::NoPersistentCookies);

    auto report = [callback, context](bool added, const QNetworkCookie &cookie) {
        const QByteArray name = cookie.name();
        const QByteArray value = cookie.value();
        const QByteArray domain = cookie.domain().toUtf8();
        const QByteArray path = cookie.path().toUtf8();
        const orbis_cookie reported{
            name.constData(),
            static_cast<size_t>(name.size()),
            value.constData(),
            static_cast<size_t>(value.size()),
            domain.constData(),
            static_cast<size_t>(domain.size()),
            path.constData(),
            static_cast<size_t>(path.size()),
            cookie.isSessionCookie() ? int64_t(-1)
                                     : int64_t(cookie.expirationDate().toSecsSinceEpoch()),
            cookie.isSecure(),
            cookie.isHttpOnly(),
        };
        callback(context, added, &reported);
    };
    QWebEngineCookieStore *store = profile->cookieStore();
    QObject::connect(store, &QWebEngineCookieStore::cookieAdded, profile,
                     [report](const QNetworkCookie &cookie) { report(true, cookie); });
    QObject::connect(store, &QWebEngineCookieStore::cookieRemoved, profile,
                     [report](const QNetworkCookie &cookie) { report(false, cookie); });
    store->loadAllCookies();

    qml->rootContext()->setContextProperty(QStringLiteral("loginProfile"), profile);
}

}
