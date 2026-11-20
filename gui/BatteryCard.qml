// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Frame {
    id: root

    property string label
    property int level: -1
    property bool charging
    property string placement

    readonly property bool known: level >= 0

    Accessible.role: Accessible.StaticText
    Accessible.name: known ? qsTr("%1 battery %2 percent").arg(label).arg(level)
                           : qsTr("%1 battery unknown").arg(label)

    ColumnLayout {
        anchors.fill: parent
        spacing: 4

        RowLayout {
            Layout.fillWidth: true
            spacing: 8

            Label {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignBaseline
                text: root.label
                opacity: 0.7
                elide: Label.ElideRight
            }
            Label {
                Layout.alignment: Qt.AlignBaseline
                text: root.known ? qsTr("%1%").arg(root.level) : "–"
                font.pixelSize: 22
            }
        }
        ProgressBar {
            Layout.fillWidth: true
            from: 0
            to: 100
            value: root.known ? root.level : 0
        }
        Label {
            font.pixelSize: 12
            opacity: 0.7
            text: {
                if (root.charging && root.known)
                    return qsTr("Charging")
                if (root.placement === "in_case")
                    return qsTr("In case")
                if (root.placement === "out")
                    return qsTr("Out of case")
                return " "
            }
        }
    }
}
