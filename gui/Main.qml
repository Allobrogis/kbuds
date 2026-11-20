// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ApplicationWindow {
    id: window

    readonly property Device device: DaemonClient.current

    width: 760
    height: 460
    minimumWidth: 420
    minimumHeight: 360
    visible: true
    title: device ? device.name : qsTr("kbuds")

    header: ToolBar {
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 16
            anchors.rightMargin: 8
            spacing: 16

            Label {
                Layout.fillWidth: true
                text: window.device ? window.device.name : qsTr("kbuds")
                font.pixelSize: 18
                elide: Label.ElideRight
            }
            ConnectionStatus {
                visible: DaemonClient.running && window.device !== null
                device: window.device
            }
            ComboBox {
                visible: DaemonClient.devices.length > 1
                model: DaemonClient.devices
                textRole: "name"
                currentIndex: DaemonClient.currentIndex
                onActivated: index => DaemonClient.currentIndex = index
            }
        }
    }

    Loader {
        anchors.fill: parent
        sourceComponent: !DaemonClient.running ? notRunning
                         : !window.device ? noDevice
                         : devicePage
    }

    Component {
        id: notRunning
        EmptyState {
            title: qsTr("The kbuds service is not running")
            hint: qsTr("Start it with: systemctl --user start kbudsd")
        }
    }

    Component {
        id: noDevice
        EmptyState {
            title: qsTr("No Pixel Buds found")
            hint: qsTr("Pair your Pixel Buds in the Bluetooth settings first.")
        }
    }

    Component {
        id: devicePage
        DevicePage {
            device: window.device
            onError: message => errorPopup.show(message)
        }
    }

    ErrorPopup {
        id: errorPopup
        x: (window.width - width) / 2
        y: window.height - height - 80
    }
}
