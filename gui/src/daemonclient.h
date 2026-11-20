// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QList>
#include <QObject>
#include <QQmlListProperty>
#include <QtQml/qqmlregistration.h>

#include "device.h"

class KbudsManagerProxy;
class QDBusServiceWatcher;

// Connection to the kbuds daemon (io.github.kbuds.Daemon on the session bus): tracks whether it
// runs, starts it through D-Bus activation, and keeps the list of devices.
class DaemonClient : public QObject
{
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON

    Q_PROPERTY(bool running READ running NOTIFY runningChanged)
    Q_PROPERTY(QQmlListProperty<Device> devices READ devicesProperty NOTIFY devicesChanged)
    Q_PROPERTY(int currentIndex READ currentIndex WRITE setCurrentIndex NOTIFY currentChanged)
    Q_PROPERTY(Device *current READ current NOTIFY currentChanged)

public:
    explicit DaemonClient(QObject *parent = nullptr);

    bool running() const { return m_running; }
    QQmlListProperty<Device> devicesProperty() { return {this, &m_devices}; }
    int currentIndex() const { return m_currentIndex; }
    void setCurrentIndex(int index);
    Device *current() const;

signals:
    void runningChanged();
    void devicesChanged();
    void currentChanged();

private:
    void setRunning(bool running);
    void loadDevices();
    void addDevice(const QDBusObjectPath &path);
    void removeDevice(const QDBusObjectPath &path);
    void clearDevices();
    void pickDefault();

    QDBusServiceWatcher *m_watcher;
    KbudsManagerProxy *m_manager;
    QList<Device *> m_devices;
    int m_currentIndex = -1;
    bool m_running = false;
};
