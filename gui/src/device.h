// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QDBusObjectPath>
#include <QObject>
#include <QStringList>
#include <QVariantMap>
#include <QtQml/qqmlregistration.h>

class KbudsDeviceProxy;
class QDBusPendingCall;

// One io.github.kbuds.Device1 object. Properties mirror the D-Bus ones and are refreshed from
// PropertiesChanged; writes are asynchronous and report failures through errorOccurred().
class Device : public QObject
{
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Devices come from DaemonClient")

    Q_PROPERTY(QString path READ path CONSTANT)
    Q_PROPERTY(QString address READ address NOTIFY changed)
    Q_PROPERTY(QString name READ name NOTIFY changed)
    Q_PROPERTY(QString model READ model NOTIFY changed)
    Q_PROPERTY(QString firmwareLeft READ firmwareLeft NOTIFY changed)
    Q_PROPERTY(QString firmwareRight READ firmwareRight NOTIFY changed)
    Q_PROPERTY(QString firmwareCase READ firmwareCase NOTIFY changed)
    Q_PROPERTY(bool bluetoothConnected READ bluetoothConnected NOTIFY changed)
    Q_PROPERTY(QString state READ state NOTIFY changed)
    Q_PROPERTY(QString primary READ primary NOTIFY changed)
    Q_PROPERTY(int batteryLeft READ batteryLeft NOTIFY changed)
    Q_PROPERTY(int batteryRight READ batteryRight NOTIFY changed)
    Q_PROPERTY(int batteryCase READ batteryCase NOTIFY changed)
    Q_PROPERTY(bool chargingLeft READ chargingLeft NOTIFY changed)
    Q_PROPERTY(bool chargingRight READ chargingRight NOTIFY changed)
    Q_PROPERTY(bool chargingCase READ chargingCase NOTIFY changed)
    Q_PROPERTY(QString placementLeft READ placementLeft NOTIFY changed)
    Q_PROPERTY(QString placementRight READ placementRight NOTIFY changed)
    Q_PROPERTY(QString anc READ anc NOTIFY changed)
    Q_PROPERTY(QStringList ancModes READ ancModes NOTIFY changed)
    Q_PROPERTY(bool multipoint READ multipoint NOTIFY changed)
    Q_PROPERTY(bool multipointSupported READ multipointSupported NOTIFY changed)
    Q_PROPERTY(bool otherHostHasAudio READ otherHostHasAudio NOTIFY changed)
    Q_PROPERTY(bool exclusive READ exclusive NOTIFY changed)
    Q_PROPERTY(QString audioProfile READ audioProfile NOTIFY changed)
    Q_PROPERTY(QString a2dpChannelMode READ a2dpChannelMode NOTIFY changed)
    Q_PROPERTY(QString audioProblem READ audioProblem NOTIFY changed)
    Q_PROPERTY(bool autoFixAudio READ autoFixAudio NOTIFY changed)
    Q_PROPERTY(bool busy READ busy NOTIFY busyChanged)

public:
    Device(const QDBusObjectPath &path, QObject *parent = nullptr);

    QString path() const { return m_path; }
    QString address() const { return str(u"Address"); }
    QString name() const { return str(u"Name"); }
    QString model() const { return str(u"Model"); }
    QString firmwareLeft() const { return str(u"FirmwareLeft"); }
    QString firmwareRight() const { return str(u"FirmwareRight"); }
    QString firmwareCase() const { return str(u"FirmwareCase"); }
    bool bluetoothConnected() const { return flag(u"BluetoothConnected"); }
    QString state() const { return str(u"State"); }
    QString primary() const { return str(u"Primary"); }
    int batteryLeft() const { return level(u"BatteryLeft"); }
    int batteryRight() const { return level(u"BatteryRight"); }
    int batteryCase() const { return level(u"BatteryCase"); }
    bool chargingLeft() const { return flag(u"ChargingLeft"); }
    bool chargingRight() const { return flag(u"ChargingRight"); }
    bool chargingCase() const { return flag(u"ChargingCase"); }
    QString placementLeft() const { return str(u"PlacementLeft"); }
    QString placementRight() const { return str(u"PlacementRight"); }
    QString anc() const { return str(u"Anc"); }
    QStringList ancModes() const;
    bool multipoint() const { return flag(u"Multipoint"); }
    bool multipointSupported() const { return flag(u"MultipointSupported"); }
    bool otherHostHasAudio() const { return flag(u"OtherHostHasAudio"); }
    bool exclusive() const { return flag(u"Exclusive"); }
    QString audioProfile() const { return str(u"AudioProfile"); }
    QString a2dpChannelMode() const { return str(u"A2dpChannelMode"); }
    QString audioProblem() const { return str(u"AudioProblem"); }
    bool autoFixAudio() const { return flag(u"AutoFixAudio"); }
    bool busy() const { return m_pending > 0; }

    Q_INVOKABLE void setAnc(const QString &mode);
    Q_INVOKABLE void setMultipoint(bool enabled);
    Q_INVOKABLE void setExclusive(bool enabled);
    Q_INVOKABLE void fixAudio();
    Q_INVOKABLE void setAutoFixAudio(bool enabled);

signals:
    void changed();
    void busyChanged();
    void errorOccurred(const QString &message);

private slots:
    void onPropertiesChanged(const QString &interface, const QVariantMap &changed, const QStringList &invalidated);

private:
    QString str(QStringView key) const;
    bool flag(QStringView key) const;
    int level(QStringView key) const;
    void refresh();
    void track(const QDBusPendingCall &call);

    QString m_path;
    KbudsDeviceProxy *m_iface;
    QVariantMap m_props;
    int m_pending = 0;
};
