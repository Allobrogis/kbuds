// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Compact connection state for the header: a coloured dot and a word.
RowLayout {
    id: root

    property Device device

    readonly property string status: {
        if (!root.device || !root.device.bluetoothConnected)
            return "off"
        return root.device.state === "connected" ? "on" : "pending"
    }

    spacing: 6

    Rectangle {
        Layout.alignment: Qt.AlignVCenter
        implicitWidth: 10
        implicitHeight: 10
        radius: 5
        color: root.status === "on" ? "#34a853"
             : root.status === "pending" ? "#fbbc04"
             : label.color
        opacity: root.status === "off" ? 0.5 : 1
        Accessible.ignored: true
    }
    Label {
        id: label
        text: {
            if (root.status === "off")
                return qsTr("Not connected")
            if (root.status === "on")
                return qsTr("Connected")
            return root.device.state === "reconnecting" ? qsTr("Reconnecting…") : qsTr("Connecting…")
        }
    }
}
