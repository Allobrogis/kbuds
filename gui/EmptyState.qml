// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    id: root

    property string title
    property string hint

    ColumnLayout {
        anchors.centerIn: parent
        width: Math.min(parent.width - 48, 360)
        spacing: 12

        Image {
            Layout.alignment: Qt.AlignHCenter
            source: "icons/io.github.kbuds.svg"
            sourceSize: Qt.size(96, 96)
            Accessible.ignored: true
        }
        Label {
            Layout.fillWidth: true
            text: root.title
            font.pixelSize: 18
            wrapMode: Label.WordWrap
            horizontalAlignment: Text.AlignHCenter
        }
        Label {
            Layout.fillWidth: true
            text: root.hint
            opacity: 0.7
            wrapMode: Label.WordWrap
            horizontalAlignment: Text.AlignHCenter
        }
    }
}
