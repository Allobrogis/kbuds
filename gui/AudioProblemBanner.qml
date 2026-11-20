// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Frame {
    id: root

    required property Device device

    ColumnLayout {
        anchors.fill: parent
        spacing: 6

        Label {
            Layout.fillWidth: true
            wrapMode: Label.WordWrap
            text: root.device.audioProblem === "dual_mono"
                  ? qsTr("The buds set up the audio stream themselves (Opus dual mono). This computer cannot take the audio back from another device.")
                  : qsTr("The buds use the headset (call) profile. If no call is active, reconnect the audio stream for better sound.")
        }
        Button {
            text: qsTr("Reconnect audio")
            enabled: !root.device.busy
            onClicked: root.device.fixAudio()
        }
    }
}
