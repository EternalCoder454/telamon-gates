import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import Telamon.Ui

// Where the next message is written: one rounded field with the Send button
// (Stop while a reply comes in) inside it at the trailing end. Enter sends,
// Shift+Enter starts a new line, Escape stops a reply.
ColumnLayout {
    id: composer

    required property var chat

    // A message went.
    signal sent

    function focusInput() {
        input.forceActiveFocus();
    }

    function submit() {
        if (composer.chat.send(input.text)) {
            input.clear();
            composer.sent();
        }
        // Back to the field after a click on Send, so Escape can stop.
        input.forceActiveFocus();
    }

    spacing: Kirigami.Units.smallSpacing

    Rectangle {
        id: field

        readonly property real inset: Kirigami.Units.smallSpacing

        Layout.fillWidth: true
        implicitHeight: Math.max(scroll.implicitHeight, action.implicitHeight) + field.inset * 2
        radius: TelamonStyle.radiusLarge
        color: TelamonStyle.control
        border.width: input.activeFocus ? 2 : 1
        border.color: input.activeFocus ? TelamonStyle.focus : TelamonStyle.controlBorder

        // A click anywhere in the field writes in it.
        TapHandler {
            onTapped: input.forceActiveFocus()
        }

        QQC2.ScrollView {
            id: scroll
            anchors.left: parent.left
            anchors.right: action.left
            anchors.verticalCenter: parent.verticalCenter
            anchors.rightMargin: Kirigami.Units.smallSpacing
            // Grows with the text, up to about ten lines, then scrolls.
            implicitHeight: Math.min(input.implicitHeight, Kirigami.Units.gridUnit * 10)
            height: implicitHeight
            QQC2.ScrollBar.vertical: TelamonScrollBar {}

            TelamonTextArea {
                id: input
                // One line to start with, not TelamonTextArea's six; the
                // field draws the frame.
                implicitHeight: contentHeight + topPadding + bottomPadding
                background: null
                wrapMode: TextEdit.Wrap
                placeholderText: qsTr("Message Telamon Gates")
                Accessible.name: qsTr("Message")

                Keys.onPressed: event => {
                    if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter) && !(event.modifiers & Qt.ShiftModifier)) {
                        // Enter sends; while a reply comes in, it waits.
                        if (!composer.chat.generating) {
                            composer.submit();
                        }
                        event.accepted = true;
                    } else if (event.key === Qt.Key_Escape && composer.chat.generating) {
                        composer.chat.stop();
                        event.accepted = true;
                    }
                }
            }
        }

        // Send, or Stop while a reply comes in: a square accent button with
        // only its symbol, at the field's trailing bottom corner.
        TelamonButton {
            id: action

            readonly property bool stopping: composer.chat.generating

            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.margins: field.inset + Kirigami.Units.smallSpacing / 2
            implicitWidth: implicitHeight
            // The row keeps a gap after the symbol for a text there is none
            // of: as much padding on the leading side keeps it centred.
            leftPadding: TelamonStyle.spacingSmall
            rightPadding: 0
            variant: action.stopping ? TelamonButton.Default : TelamonButton.Prominent
            symbol: action.stopping ? Symbols.Stop : Symbols.ArrowUpward
            enabled: action.stopping || (input.text.trim().length > 0 && !composer.chat.loading)
            Accessible.name: action.stopping ? qsTr("Stop") : qsTr("Send")
            onClicked: {
                if (action.stopping) {
                    composer.chat.stop();
                    input.forceActiveFocus();
                } else {
                    composer.submit();
                }
            }
        }
    }

    // A key and what it does, in a small box.
    component KeyHint: Rectangle {
        id: hint
        required property string keys
        required property string action

        implicitWidth: hintRow.implicitWidth + Kirigami.Units.smallSpacing * 2
        implicitHeight: hintRow.implicitHeight + Kirigami.Units.smallSpacing
        radius: TelamonStyle.radiusSmall
        color: "transparent"
        border.width: 1
        border.color: TelamonStyle.controlBorder
        Accessible.role: Accessible.StaticText
        Accessible.name: qsTr("%1: %2").arg(hint.keys).arg(hint.action)

        RowLayout {
            id: hintRow
            anchors.centerIn: parent
            spacing: Kirigami.Units.smallSpacing

            TelamonShortcutLabel {
                sequence: hint.keys
            }
            TelamonLabel {
                textStyle: TelamonLabel.Caption
                textFormat: Text.PlainText
                text: hint.action
            }
        }
    }

    RowLayout {
        Layout.alignment: Qt.AlignHCenter
        spacing: Kirigami.Units.largeSpacing

        KeyHint {
            keys: "Enter"
            action: qsTr("Send")
        }
        KeyHint {
            keys: "Shift+Enter"
            action: qsTr("New Line")
        }
        TelamonLabel {
            textStyle: TelamonLabel.Caption
            text: qsTr("Always double-check the answer.")
        }
    }
}
