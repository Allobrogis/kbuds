// SPDX-License-Identifier: GPL-3.0-or-later
#include "device.h"

#include "device1_interface.h"

#include <QDBusArgument>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusPendingCallWatcher>
#include <QDBusPendingReply>
#include <QDBusVariant>

namespace {
const QString kService = QStringLiteral("io.github.kbuds.Daemon");
const QString kInterface = QStringLiteral("io.github.kbuds.Device1");
const QString kProperties = QStringLiteral("org.freedesktop.DBus.Properties");

// Values inside a{sv} may arrive as QDBusArgument for container types.
QVariant unwrap(const QVariant &v)
{
    if (v.userType() == qMetaTypeId<QDBusVariant>())
        return unwrap(v.value<QDBusVariant>().variant());
    if (v.userType() == qMetaTypeId<QDBusArgument>()) {
        const auto arg = v.value<QDBusArgument>();
        if (arg.currentSignature() == QLatin1String("as"))
            return qdbus_cast<QStringList>(arg);
    }
    return v;
}
} // namespace

Device::Device(const QDBusObjectPath &path, QObject *parent)
    : QObject(parent)
    , m_path(path.path())
    , m_iface(new KbudsDeviceProxy(kService, m_path, QDBusConnection::sessionBus(), this))
{
    QDBusConnection::sessionBus().connect(kService, m_path, kProperties, QStringLiteral("PropertiesChanged"), this,
                                          SLOT(onPropertiesChanged(QString, QVariantMap, QStringList)));
    refresh();
}

QString Device::str(QStringView key) const
{
    return m_props.value(key.toString()).toString();
}

bool Device::flag(QStringView key) const
{
    return m_props.value(key.toString()).toBool();
}

int Device::level(QStringView key) const
{
    return m_props.value(key.toString(), -1).toInt();
}

QStringList Device::ancModes() const
{
    return m_props.value(QStringLiteral("AncModes")).toStringList();
}

void Device::refresh()
{
    auto msg = QDBusMessage::createMethodCall(kService, m_path, kProperties, QStringLiteral("GetAll"));
    msg << kInterface;
    auto *watcher = new QDBusPendingCallWatcher(QDBusConnection::sessionBus().asyncCall(msg), this);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher *w) {
        QDBusPendingReply<QVariantMap> reply = *w;
        w->deleteLater();
        if (reply.isError())
            return;
        const QVariantMap all = reply.value();
        for (auto it = all.cbegin(); it != all.cend(); ++it)
            m_props.insert(it.key(), unwrap(it.value()));
        emit changed();
    });
}

void Device::onPropertiesChanged(const QString &interface, const QVariantMap &changedProps,
                                 const QStringList &invalidated)
{
    if (interface != kInterface)
        return;
    for (auto it = changedProps.cbegin(); it != changedProps.cend(); ++it)
        m_props.insert(it.key(), unwrap(it.value()));
    if (!invalidated.isEmpty()) {
        refresh();
        return;
    }
    emit changed();
}

void Device::track(const QDBusPendingCall &call)
{
    ++m_pending;
    emit busyChanged();
    auto *watcher = new QDBusPendingCallWatcher(call, this);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher *w) {
        w->deleteLater();
        --m_pending;
        emit busyChanged();
        if (w->isError()) {
            emit errorOccurred(w->error().message());
            // Re-sync controls that optimistically moved (switches) with the real state.
            emit changed();
        }
    });
}

void Device::setAnc(const QString &mode)
{
    track(m_iface->SetAnc(mode));
}

void Device::setMultipoint(bool enabled)
{
    track(m_iface->SetMultipoint(enabled));
}

void Device::setExclusive(bool enabled)
{
    track(m_iface->SetExclusive(enabled));
}

void Device::fixAudio()
{
    track(m_iface->FixAudio());
}

void Device::setAutoFixAudio(bool enabled)
{
    auto msg = QDBusMessage::createMethodCall(kService, m_path, kProperties, QStringLiteral("Set"));
    msg << kInterface << QStringLiteral("AutoFixAudio") << QVariant::fromValue(QDBusVariant(enabled));
    track(QDBusConnection::sessionBus().asyncCall(msg));
}
