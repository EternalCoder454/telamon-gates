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

        readonly property real inset: TelamonStyle.spacingSmall
        // One line of text with the field's padding: the button centres on
        // it, so it sits level with the first line and stays at the bottom
        // as the text grows.
        readonly property real line: Math.ceil(metrics.height) + input.topPadding + input.bottomPadding

        Layout.fillWidth: true
        implicitHeight: Math.max(scroll.implicitHeight, field.line) + field.inset * 2
        radius: TelamonStyle.radiusLarge
        color: hover.hovered && !input.activeFocus ? Qt.tint(TelamonStyle.control, TelamonStyle.hover) : TelamonStyle.control
        // A hairline, the accent while writing: the field is the page's main
        // control, and the violet sits with the Send button beside it.
        border.width: 1
        border.color: input.activeFocus ? TelamonStyle.accent : TelamonStyle.controlBorder

        FontMetrics {
            id: metrics
            font: input.font
        }
        HoverHandler {
            id: hover
            cursorShape: Qt.IBeamCursor
        }

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
                topPadding: TelamonStyle.spacing
                bottomPadding: TelamonStyle.spacing
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
        // only its symbol, at the field's trailing end. With nothing to send
        // it stays accent, faded, rather than turning into a grey outline.
        TelamonButton {
            id: action

            readonly property bool stopping: composer.chat.generating
            readonly property bool ready: input.text.trim().length > 0 && !composer.chat.loading

            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.rightMargin: field.inset + Math.round((field.line - height) / 2)
            anchors.bottomMargin: field.inset + Math.round((field.line - height) / 2)
            implicitWidth: implicitHeight
            // Only the symbol, centred: Row lays out no gap for the empty
            // label (a zero-width item), so no padding either side.
            leftPadding: 0
            rightPadding: 0
            // The symbol font draws its glyphs about a pixel above the
            // middle of their line: this lowers them to the button's centre.
            topPadding: 2
            bottomPadding: 0
            variant: TelamonButton.Prominent
            symbol: action.stopping ? Symbols.Stop : Symbols.ArrowUpward
            opacity: action.stopping || action.ready ? 1 : 0.55
            hoverEnabled: action.stopping || action.ready
            focusPolicy: Qt.NoFocus
            Accessible.name: action.stopping ? qsTr("Stop") : qsTr("Send")
            onClicked: {
                if (action.stopping) {
                    composer.chat.stop();
                    input.forceActiveFocus();
                } else if (action.ready) {
                    composer.submit();
                } else {
                    input.forceActiveFocus();
                }
            }

            Behavior on opacity {
                NumberAnimation {
                    duration: TelamonStyle.durationShort
                }
            }
        }
    }

    // A key and what it does, as a small pill: "Enter → Send".
    component KeyHint: Rectangle {
        id: hint
        required property string keys
        required property string action

        implicitWidth: hintRow.implicitWidth + TelamonStyle.spacing * 2
        implicitHeight: hintRow.implicitHeight + TelamonStyle.spacingXSmall * 2
        radius: TelamonStyle.radiusSmall
        color: Qt.alpha(Kirigami.Theme.textColor, 0.06)
        Accessible.role: Accessible.StaticText
        Accessible.name: qsTr("%1: %2").arg(hint.keys).arg(hint.action)

        Row {
            id: hintRow
            anchors.centerIn: parent
            spacing: TelamonStyle.spacingSmall

            TelamonLabel {
                textStyle: TelamonLabel.Caption
                textFormat: Text.PlainText
                font.weight: Font.DemiBold
                text: hint.keys
            }
            TelamonLabel {
                textStyle: TelamonLabel.Caption
                textFormat: Text.PlainText
                text: "→"
            }
            TelamonLabel {
                textStyle: TelamonLabel.Caption
                textFormat: Text.PlainText
                text: hint.action
            }
        }
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: TelamonStyle.spacing

        // How the reply is written: Auto (SystemOne picks per message; without
        // it, as the last reply) or a mode pinned for the conversation.
        TelamonSegmentedControl {
            id: modes

            // [id, label, tooltip]
            readonly property var choices: [
                ["auto", qsTr("Auto"), composer.chat.systemOneReady ? qsTr("SystemOne picks Chat, Story or Code for each message") : qsTr("Chat, until SystemOne has a decision model (Settings)")],
                ["chat", qsTr("Chat"), qsTr("Questions, advice and everyday talk")],
                ["story", qsTr("Story"), qsTr("Creative writing: warmer, and keeps to your story")],
                ["code", qsTr("Code"), qsTr("Programming: careful and precise")],
                ["agent", qsTr("Agent"), qsTr("Works in a folder with tools: reads files, and asks before it edits them or runs commands")]
            ]

            model: modes.choices.map(c => ({
                        text: c[1],
                        toolTip: c[2]
                    }))
            // None when the conversation is in one of the user's own modes.
            currentIndex: modes.choices.findIndex(c => c[0] === composer.chat.mode)
            // Not while a reply runs: an agent keeps its mode and folder.
            enabled: !composer.chat.generating
            onActivated: index => composer.chat.chooseMode(modes.choices[index][0])
            Accessible.name: qsTr("Mode")
        }

        // The user's own modes (Settings), after the built-in four.
        TelamonComboBox {
            id: mine
            readonly property var ids: composer.chat.modeIds.slice(4)
            visible: mine.ids.length > 0
            enabled: !composer.chat.generating
            Layout.preferredWidth: Kirigami.Units.gridUnit * 8
            model: [qsTr("Your Modes")].concat(composer.chat.modeNames.slice(4))
            currentIndex: mine.ids.indexOf(composer.chat.mode) + 1
            onActivated: index => {
                if (index > 0) {
                    composer.chat.chooseMode(mine.ids[index - 1]);
                }
            }
            Accessible.name: qsTr("Your Modes")
        }

        // Asks for no width of its own, so a narrow window keeps the field (and
        // its Send button) inside; the reminder goes first when there's no room.
        Item {
            Layout.fillWidth: true
            implicitHeight: hints.implicitHeight
            clip: true

            RowLayout {
                id: hints
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                spacing: TelamonStyle.spacing

                KeyHint {
                    id: sendHint
                    // Whole or not at all: never half a pill.
                    visible: hints.parent.width >= sendHint.implicitWidth
                    //: The Enter key, as printed on it
                    keys: qsTr("Enter")
                    action: qsTr("Send")
                }
                KeyHint {
                    id: lineHint
                    visible: hints.parent.width >= sendHint.implicitWidth + lineHint.implicitWidth + hints.spacing
                    //: The keys Shift and Enter together
                    keys: qsTr("Shift+Enter")
                    action: qsTr("New Line")
                }
                TelamonLabel {
                    id: reminder
                    Layout.leftMargin: TelamonStyle.spacingSmall
                    visible: hints.parent.width >= sendHint.implicitWidth + lineHint.implicitWidth + reminder.implicitWidth + hints.spacing * 2 + TelamonStyle.spacingSmall
                    textStyle: TelamonLabel.Caption
                    text: qsTr("Always double-check the answer.")
                }
            }
        }
    }
}
