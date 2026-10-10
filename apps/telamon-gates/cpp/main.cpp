// Starts Qt, makes the app single-instance and loads the window. All app
// logic is in Rust (src/); this file only glues.
#include <KDBusService>
#include <KWindowSystem>

#include <QApplication>
#include <QCommandLineParser>
#include <QPointer>
#include <QQmlApplicationEngine>
#include <QQmlEngine>
#include <QQuickWindow>
#include <QSGRendererInterface>
#include <utility>

// Rust, see src/lib.rs.
struct TelamonObjects {
    void *chat;
    void *library;
    void *vram;
    void *models;
    void *fleet;
    void *workbench;
};
extern "C" TelamonObjects telamon_objects_new();
// telamon-framework-ui (include/telamon/app.h), linked in with the Rust library.
extern "C" void telamon_app_init();
extern "C" void telamon_app_ready();

int main(int argc, char *argv[])
{
    // Before QApplication: the journal logger, the crash hooks, the app ID as
    // organization domain and application name (together the single-instance
    // D-Bus name net.eterneon.telamon.gates) and desktop file name, the
    // version, and the org.kde.desktop style.
    telamon_app_init();

    // Draw on the CPU (Qt Quick's software backend): a chat window is text,
    // and the GPU path loads Mesa and LLVM. Telamon.Ui moves HiDPI screens to
    // the GPU itself. QT_QUICK_BACKEND still overrides.
    if (qEnvironmentVariableIsEmpty("QT_QUICK_BACKEND")) {
        QQuickWindow::setGraphicsApi(QSGRendererInterface::Software);
    }

    QApplication app(argc, argv);
    // The display name, the window icon, and what Telamon.Ui's TelamonApp shows.
    telamon_app_ready();

    QCommandLineParser parser;
    parser.addHelpOption();
    parser.addVersionOption();
    parser.process(app);

    // One instance per session: a second launch asks this one to show its
    // window (activateRequested) and exits.
    KDBusService service(KDBusService::Unique);

    // Made in Rust; main() owns them, so the QML engine must never delete one.
    const TelamonObjects made = telamon_objects_new();
    const std::pair<const char *, void *> objects[] = {
        {"chat", made.chat},
        {"library", made.library},
        {"vram", made.vram},
        {"models", made.models},
        {"fleet", made.fleet},
        {"workbench", made.workbench},
    };
    QVariantMap initial;
    for (const auto &[name, object] : objects) {
        auto *o = static_cast<QObject *>(object);
        QQmlEngine::setObjectOwnership(o, QQmlEngine::CppOwnership);
        initial.insert(QLatin1String(name), QVariant::fromValue(o));
    }

    int rc = 0;
    {
        QQmlApplicationEngine engine;
        engine.setInitialProperties(initial);
        QObject::connect(&engine, &QQmlApplicationEngine::objectCreationFailed, &app, [] { QCoreApplication::exit(1); }, Qt::QueuedConnection);
        engine.loadFromModule(QStringLiteral("net.eterneon.telamon.gates"), QStringLiteral("Main"));

        QPointer<QQuickWindow> window = qobject_cast<QQuickWindow *>(engine.rootObjects().value(0));
        if (window) {
            QObject::connect(&service, &KDBusService::activateRequested, window, [window](const QStringList &, const QString &) {
                // On Wayland, KWin only lets a window take focus with the
                // second launch's activation token.
                KWindowSystem::updateStartupId(window);
                window->show();
                window->raise();
                KWindowSystem::activateWindow(window);
            });
            rc = app.exec();
        } else {
            rc = 1;
        }
    }
    // The chat first: a reply under way posts to it, and it posts to the list.
    for (const auto &[name, object] : objects) {
        delete static_cast<QObject *>(object);
    }
    return rc;
}
