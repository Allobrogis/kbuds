// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// A switch with a title and a one-line explanation. `checked` follows the bound value; a toggle
// asks for the change and the daemon's PropertiesChanged updates the switch.
RowLayout {
    id: root

    property string text
    property string description
    property bool checked

    signal toggled(bool checked)

    spacing: 8

    ColumnLayout {
        Layout.fillWidth: true
        spacing: 0

        Label {
            Layout.fillWidth: true
            text: root.text
        }
        Label {
            Layout.fillWidth: true
            text: root.description
            font.pixelSize: 12
            opacity: 0.7
            wrapMode: Label.WordWrap
        }
    }
    Switch {
        checked: root.checked
        Accessible.name: root.text
        onToggled: {
            const wanted = checked
            checked = Qt.binding(() => root.checked)
            root.toggled(wanted)
        }
    }
}
