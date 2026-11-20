// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Landscape layout: battery strip on top, noise control and audio side by side (stacked when the
// window is narrow), model and firmware in a footer line.
ScrollView {
    id: root

    required property Device device
    readonly property bool sessionUp: root.device.state === "connected"
    readonly property bool twoColumns: root.availableWidth >= 620 && noiseControl.visible

    signal error(string message)

    function modelName(model) {
        switch (model) {
        case "pro": return qsTr("Pixel Buds Pro")
        case "pro2": return qsTr("Pixel Buds Pro 2")
        case "2a": return qsTr("Pixel Buds 2a")
        }
        return ""
    }

    contentWidth: availableWidth
    contentHeight: page.implicitHeight + 32

    Connections {
        target: root.device
        function onErrorOccurred(message) { root.error(message) }
    }

    ColumnLayout {
        id: page
        x: 16
        y: 16
        width: root.availableWidth - 32
        spacing: 12

        ConnectionBanner {
            Layout.fillWidth: true
            device: root.device
            visible: !root.sessionUp
        }

        RowLayout {
            Layout.fillWidth: true
            visible: root.sessionUp
            spacing: 12

            BatteryCard {
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                label: qsTr("Left")
                level: root.device.batteryLeft
                charging: root.device.chargingLeft
                placement: root.device.placementLeft
            }
            BatteryCard {
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                label: qsTr("Right")
                level: root.device.batteryRight
                charging: root.device.chargingRight
                placement: root.device.placementRight
            }
            BatteryCard {
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                label: qsTr("Case")
                level: root.device.batteryCase
                charging: root.device.chargingCase
                placement: ""
            }
        }

        GridLayout {
            Layout.fillWidth: true
            visible: noiseControl.visible || audioPanel.visible
            columns: root.twoColumns ? 2 : 1
            columnSpacing: 12
            rowSpacing: 12

            NoiseControl {
                id: noiseControl
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.preferredWidth: 1
                device: root.device
                visible: root.sessionUp && root.device.ancModes.length > 0
            }

            AudioPanel {
                id: audioPanel
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.preferredWidth: 1
                device: root.device
                sessionUp: root.sessionUp
                visible: root.device.bluetoothConnected
            }
        }

        Label {
            Layout.fillWidth: true
            visible: root.sessionUp
            opacity: 0.6
            font.pixelSize: 12
            wrapMode: Label.WordWrap
            text: {
                const firmware = qsTr("Firmware: left %1, right %2, case %3")
                    .arg(root.device.firmwareLeft || "–")
                    .arg(root.device.firmwareRight || "–")
                    .arg(root.device.firmwareCase || "–")
                const model = root.modelName(root.device.model)
                return model ? qsTr("%1 · %2").arg(model).arg(firmware) : firmware
            }
        }
    }
}
