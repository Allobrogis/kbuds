// SPDX-License-Identifier: GPL-3.0-or-later
#include <QGuiApplication>
#include <QIcon>
#include <QQmlApplicationEngine>
#include <QQuickStyle>

int main(int argc, char *argv[])
{
    QGuiApplication app(argc, argv);
    QGuiApplication::setApplicationName(QStringLiteral("kbuds"));
    QGuiApplication::setApplicationDisplayName(QStringLiteral("kbuds"));
    QGuiApplication::setApplicationVersion(QStringLiteral(KBUDS_VERSION));
    QGuiApplication::setDesktopFileName(QStringLiteral("io.github.kbuds"));
    QGuiApplication::setWindowIcon(QIcon::fromTheme(QStringLiteral("io.github.kbuds")));

    // Material is the design. Without this, a desktop platform theme
    // (KDE: org.kde.breeze) outranks qtquickcontrols2.conf. An explicit QT_QUICK_CONTROLS_STYLE or
    // -style still wins.
    const bool styleArg = QCoreApplication::arguments().contains(QStringLiteral("-style"))
        || QCoreApplication::arguments().contains(QStringLiteral("--style"));
    if (!styleArg && qEnvironmentVariableIsEmpty("QT_QUICK_CONTROLS_STYLE"))
        QQuickStyle::setStyle(QStringLiteral("Material"));

    QQmlApplicationEngine engine;
    QObject::connect(&engine, &QQmlApplicationEngine::objectCreationFailed, &app,
                     [] { QCoreApplication::exit(1); }, Qt::QueuedConnection);
    engine.loadFromModule("Kbuds", "Main");
    return app.exec();
}
