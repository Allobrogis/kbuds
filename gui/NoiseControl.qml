// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Frame {
    id: root

    required property Device device
    readonly property bool unavailable: root.device.anc === "unavailable"

    function modeLabel(mode) {
        switch (mode) {
        case "off": return qsTr("Off")
        case "active": return qsTr("Noise Cancellation")
        case "aware": return qsTr("Transparency")
        case "adaptive": return qsTr("Adaptive")
        }
        return mode
    }

    ColumnLayout {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        spacing: 4

        Label {
            text: qsTr("Noise control")
            font.pixelSize: 16
        }
        Repeater {
            model: root.device.ancModes
            delegate: RadioButton {
                required property string modelData
                Layout.fillWidth: true
                text: root.modeLabel(modelData)
                // Bound to the buds' state, not exclusive by itself: the daemon confirms the write
                // and the binding moves the check.
                autoExclusive: false
                checked: root.device.anc === modelData
                enabled: !root.unavailable && !root.device.busy
                onClicked: {
                    checked = Qt.binding(() => root.device.anc === modelData)
                    if (root.device.anc !== modelData)
                        root.device.setAnc(modelData)
                }
            }
        }
        Label {
            Layout.fillWidth: true
            visible: root.unavailable
            wrapMode: Label.WordWrap
            opacity: 0.7
            text: qsTr("Put at least one bud in your ear to change noise control.")
        }
    }
}
