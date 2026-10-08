import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import Telamon.Ui

// Where the next message is written. Enter sends, Shift+Enter starts a new
// line, Escape stops a reply coming in.
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
    }

    spacing: Kirigami.Units.smallSpacing

    RowLayout {
        Layout.fillWidth: true
        spacing: Kirigami.Units.smallSpacing

        QQC2.ScrollView {
            Layout.fillWidth: true
            // Grows with the text, up to about ten lines, then scrolls.
            Layout.preferredHeight: Math.min(input.implicitHeight, Kirigami.Units.gridUnit * 10)
            QQC2.ScrollBar.vertical: TelamonScrollBar {}

            TelamonTextArea {
                id: input
                // One line to start with, not TelamonTextArea's six.
                implicitHeight: contentHeight + topPadding + bottomPadding
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

        PrimaryButton {
            Layout.alignment: Qt.AlignBottom
            visible: !composer.chat.generating
            enabled: input.text.trim().length > 0 && !composer.chat.loading
            symbol: Symbols.ArrowUpward
            text: qsTr("Send")
            onClicked: composer.submit()
        }
        SecondaryButton {
            Layout.alignment: Qt.AlignBottom
            visible: composer.chat.generating
            symbol: Symbols.Stop
            text: qsTr("Stop")
            onClicked: composer.chat.stop()
        }
    }

    TelamonLabel {
        Layout.alignment: Qt.AlignHCenter
        textStyle: TelamonLabel.Caption
        text: qsTr("Enter to send, Shift+Enter for a new line. Replies can be wrong: check what matters.")
    }
}
