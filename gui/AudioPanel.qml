// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Frame {
    id: root

    required property Device device
    property bool sessionUp

    ColumnLayout {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        spacing: 4

        Label {
            text: qsTr("Audio")
            font.pixelSize: 16
        }

        AudioProblemBanner {
            Layout.fillWidth: true
            device: root.device
            visible: root.device.audioProblem !== ""
        }

        Label {
            Layout.fillWidth: true
            visible: root.sessionUp
            wrapMode: Label.WordWrap
            text: root.device.otherHostHasAudio ? qsTr("Another device is playing on the buds")
                                                : qsTr("The buds play this computer's audio")
        }

        SettingSwitch {
            Layout.fillWidth: true
            visible: root.sessionUp && root.device.multipointSupported
            text: qsTr("Multipoint")
            description: qsTr("Stay connected to two devices at once")
            checked: root.device.multipoint
            enabled: !root.device.busy
            onToggled: root.device.setMultipoint(checked)
        }

        SettingSwitch {
            Layout.fillWidth: true
            visible: root.sessionUp
            text: qsTr("PC only")
            description: qsTr("Disconnect the other device until this is turned off")
            checked: root.device.exclusive
            enabled: !root.device.busy
            onToggled: root.device.setExclusive(checked)
        }

        SettingSwitch {
            Layout.fillWidth: true
            text: qsTr("Fix audio automatically")
            description: qsTr("Reconnect the audio stream when it is set up in a way that stops this computer from taking the audio back")
            checked: root.device.autoFixAudio
            onToggled: root.device.setAutoFixAudio(checked)
        }
    }
}
