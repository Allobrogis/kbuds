// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Frame {
    id: root

    required property Device device


    RowLayout {
        anchors.fill: parent
        spacing: 12

        BusyIndicator {
            running: root.visible && root.device.bluetoothConnected
            visible: running
            Layout.preferredWidth: 32
            Layout.preferredHeight: 32
        }
        Label {
            Layout.fillWidth: true
            wrapMode: Label.WordWrap
            text: {
                if (!root.device.bluetoothConnected)
                    return qsTr("Not connected. Take the buds out of the case, or connect them in the Bluetooth settings.")
                if (root.device.state === "reconnecting")
                    return qsTr("Reconnecting to the buds…")
                return qsTr("Connecting to the buds…")
            }
        }
    }
}
