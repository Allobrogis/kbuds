// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtQuick.Controls

Popup {
    id: root

    property alias text: message.text

    function show(msg) {
        text = msg
        open()
        hideTimer.restart()
    }

    width: Math.min(parent ? parent.width - 32 : 400, 420)
    closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside

    Label {
        id: message
        width: parent.width
        wrapMode: Label.WordWrap
    }

    Timer {
        id: hideTimer
        interval: 5000
        onTriggered: root.close()
    }
}
