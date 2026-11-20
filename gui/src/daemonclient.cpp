// SPDX-License-Identifier: GPL-3.0-or-later
#include "daemonclient.h"

#include "manager1_interface.h"

#include <QDBusArgument>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusPendingCallWatcher>
#include <QDBusPendingReply>
#include <QDBusServiceWatcher>
#include <QDBusVariant>

namespace {
const QString kService = QStringLiteral("io.github.kbuds.Daemon");
const QString kManagerPath = QStringLiteral("/io/github/kbuds");
} // namespace

DaemonClient::DaemonClient(QObject *parent)
    : QObject(parent)
    , m_watcher(new QDBusServiceWatcher(kService, QDBusConnection::sessionBus(),
                                        QDBusServiceWatcher::WatchForOwnerChange, this))
    , m_manager(new KbudsManagerProxy(kService, kManagerPath, QDBusConnection::sessionBus(), this))
{
    connect(m_watcher, &QDBusServiceWatcher::serviceRegistered, this, [this] {
        setRunning(true);
        loadDevices();
    });
    connect(m_watcher, &QDBusServiceWatcher::serviceUnregistered, this, [this] {
        setRunning(false);
        clearDevices();
    });
    connect(m_manager, &KbudsManagerProxy::DeviceAdded, this, &DaemonClient::addDevice);
    connect(m_manager, &KbudsManagerProxy::DeviceRemoved, this, &DaemonClient::removeDevice);

    // Starts the daemon through D-Bus activation if needed (it is also a systemd user service).
    auto msg = QDBusMessage::createMethodCall(QStringLiteral("org.freedesktop.DBus"),
                                              QStringLiteral("/org/freedesktop/DBus"),
                                              QStringLiteral("org.freedesktop.DBus"),
                                              QStringLiteral("StartServiceByName"));
    msg << kService << 0u;
    auto *watcher = new QDBusPendingCallWatcher(QDBusConnection::sessionBus().asyncCall(msg), this);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher *w) {
        w->deleteLater();
        if (!w->isError()) {
            setRunning(true);
            loadDevices();
        }
    });
}

void DaemonClient::setRunning(bool running)
{
    if (m_running == running)
        return;
    m_running = running;
    emit runningChanged();
}

void DaemonClient::loadDevices()
{
    auto msg = QDBusMessage::createMethodCall(kService, kManagerPath, QStringLiteral("org.freedesktop.DBus.Properties"),
                                              QStringLiteral("Get"));
    msg << QStringLiteral("io.github.kbuds.Manager1") << QStringLiteral("Devices");
    auto *watcher = new QDBusPendingCallWatcher(QDBusConnection::sessionBus().asyncCall(msg), this);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher *w) {
        QDBusPendingReply<QDBusVariant> reply = *w;
        w->deleteLater();
        if (reply.isError())
            return;
        const auto paths = qdbus_cast<QList<QDBusObjectPath>>(reply.value().variant().value<QDBusArgument>());
        for (const auto &p : paths)
            addDevice(p);
    });
}

void DaemonClient::addDevice(const QDBusObjectPath &path)
{
    for (const Device *d : std::as_const(m_devices)) {
        if (d->path() == path.path())
            return;
    }
    auto *device = new Device(path, this);
    // The default selection follows connection changes until the user picks a device.
    connect(device, &Device::changed, this, &DaemonClient::pickDefault);
    m_devices.append(device);
    emit devicesChanged();
    pickDefault();
}

void DaemonClient::removeDevice(const QDBusObjectPath &path)
{
    for (qsizetype i = 0; i < m_devices.size(); ++i) {
        if (m_devices.at(i)->path() != path.path())
            continue;
        Device *d = m_devices.takeAt(i);
        d->deleteLater();
        emit devicesChanged();
        if (m_currentIndex >= m_devices.size() || m_currentIndex == i) {
            m_currentIndex = -1;
            pickDefault();
            emit currentChanged();
        } else if (m_currentIndex > i) {
            --m_currentIndex;
            emit currentChanged();
        }
        return;
    }
}

void DaemonClient::clearDevices()
{
    qDeleteAll(m_devices);
    m_devices.clear();
    m_currentIndex = -1;
    emit devicesChanged();
    emit currentChanged();
}

void DaemonClient::pickDefault()
{
    if (m_currentIndex >= 0 || m_devices.isEmpty())
        return;
    int best = 0;
    for (qsizetype i = 0; i < m_devices.size(); ++i) {
        if (m_devices.at(i)->bluetoothConnected()) {
            best = int(i);
            break;
        }
    }
    // Wait for the first properties before choosing among several devices.
    if (m_devices.size() > 1 && m_devices.at(best)->address().isEmpty())
        return;
    m_currentIndex = best;
    emit currentChanged();
}

void DaemonClient::setCurrentIndex(int index)
{
    if (index == m_currentIndex || index < 0 || index >= m_devices.size())
        return;
    m_currentIndex = index;
    emit currentChanged();
}

Device *DaemonClient::current() const
{
    return m_currentIndex >= 0 && m_currentIndex < m_devices.size() ? m_devices.at(m_currentIndex) : nullptr;
}
