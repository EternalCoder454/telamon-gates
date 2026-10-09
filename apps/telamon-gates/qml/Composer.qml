pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Dialogs
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

    // Files to go with the next message, each removable.
    Flow {
        Layout.fillWidth: true
        visible: composer.chat.pendingNames.length > 0
        spacing: TelamonStyle.spacingSmall

        Repeater {
            model: composer.chat.pendingNames

            TelamonChip {
                required property int index
                required property string modelData
                text: modelData
                symbol: (composer.chat.pendingImages[index] ?? "").length > 0 ? Symbols.Image : Symbols.Description
                maximumWidth: Kirigami.Units.gridUnit * 14
                closable: true
                onCloseRequested: composer.chat.removeAttachment(index)
            }
        }
    }

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

        // Files and pictures to send with the message: at the field's
        // leading end, across from Send.
        ToolbarButton {
            id: attach
            anchors.left: parent.left
            anchors.bottom: parent.bottom
            anchors.leftMargin: field.inset + Math.round((field.line - height) / 2)
            anchors.bottomMargin: field.inset + Math.round((field.line - height) / 2)
            symbol: Symbols.AttachFile
            text: qsTr("Attach Files…")
            focusable: true
            onClicked: picker.open()
        }

        QQC2.ScrollView {
            id: scroll
            anchors.left: attach.right
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
                // A pause in the typing gets the model ready (see prepare).
                onTextChanged: prepTimer.restart()

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
            readonly property bool ready: (input.text.trim().length > 0 || composer.chat.pendingNames.length > 0) && !composer.chat.loading

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

        // How the reply is written, in one button: Auto (SystemOne picks per
        // message; without it, as the last reply), a built-in mode, or one
        // of the user's own. Not changed while a reply runs.
        SecondaryButton {
            id: modeButton

            // [id, label, symbol, tooltip] for the built-in choices.
            readonly property var builtIn: [
                ["auto", qsTr("Auto"), Symbols.AutoAwesome, composer.chat.systemOneReady ? qsTr("SystemOne picks Chat, Story or Code for each message") : qsTr("Chat, until SystemOne has a decision model (Settings)")],
                ["chat", qsTr("Chat"), Symbols.Chat, qsTr("Questions, advice and everyday talk")],
                ["story", qsTr("Story"), Symbols.AutoStories, qsTr("Creative writing: warmer, and keeps to your story")],
                ["code", qsTr("Code"), Symbols.Code, qsTr("Programming: careful and precise")],
                ["agent", qsTr("Agent"), Symbols.SmartToy, qsTr("Works in a folder with tools: reads files, and asks before it edits them or runs commands")],
                ["research", qsTr("Deep Research"), Symbols.TravelExplore, qsTr("Searches the web and reads pages to write a report with its sources")]
            ]
            readonly property int ownAt: composer.chat.modeIds.indexOf(composer.chat.mode)
            readonly property var current: modeButton.builtIn.find(c => c[0] === composer.chat.mode) ?? ["", modeButton.ownAt >= 0 ? composer.chat.modeNames[modeButton.ownAt] : qsTr("Chat"), Symbols.EditNote, ""]

            text: modeButton.current[1]
            symbol: modeButton.current[2]
            enabled: !composer.chat.generating
            Accessible.name: qsTr("Mode: %1").arg(modeButton.current[1])
            onClicked: modeMenu.popup(modeButton, 0, -modeMenu.implicitHeight - TelamonStyle.spacingSmall)

            ContextMenu {
                id: modeMenu

                ContextMenuItem {
                    text: modeButton.builtIn[0][1]
                    symbol: modeButton.builtIn[0][2]
                    radio: true
                    checked: composer.chat.mode === "auto"
                    onTriggered: composer.chat.chooseMode("auto")
                }
                ContextMenuItem {
                    text: modeButton.builtIn[1][1]
                    symbol: modeButton.builtIn[1][2]
                    radio: true
                    checked: composer.chat.mode === "chat"
                    onTriggered: composer.chat.chooseMode("chat")
                }
                ContextMenuItem {
                    text: modeButton.builtIn[2][1]
                    symbol: modeButton.builtIn[2][2]
                    radio: true
                    checked: composer.chat.mode === "story"
                    onTriggered: composer.chat.chooseMode("story")
                }
                ContextMenuItem {
                    text: modeButton.builtIn[3][1]
                    symbol: modeButton.builtIn[3][2]
                    radio: true
                    checked: composer.chat.mode === "code"
                    onTriggered: composer.chat.chooseMode("code")
                }
                ContextMenuItem {
                    text: modeButton.builtIn[4][1]
                    symbol: modeButton.builtIn[4][2]
                    radio: true
                    checked: composer.chat.mode === "agent"
                    onTriggered: composer.chat.chooseMode("agent")
                }
                ContextMenuItem {
                    text: modeButton.builtIn[5][1]
                    symbol: modeButton.builtIn[5][2]
                    radio: true
                    // Needs Web Search and a model that can call tools.
                    enabled: composer.chat.researchNote.length === 0
                    checked: composer.chat.mode === "research"
                    onTriggered: composer.chat.chooseMode("research")
                }
                // What Deep Research is missing, when it is.
                ContextMenuItem {
                    visible: composer.chat.researchNote.length > 0
                    enabled: false
                    text: composer.chat.researchNote
                    symbol: Symbols.Info
                }
                ContextMenuSeparator {
                    visible: composer.chat.modeIds.length > 5
                }
            }

            // The user's own modes, after the separator.
            Instantiator {
                model: composer.chat.modeIds.slice(5)
                delegate: ContextMenuItem {
                    required property int index
                    required property string modelData
                    text: composer.chat.modeNames[index + 5] ?? ""
                    symbol: Symbols.EditNote
                    radio: true
                    checked: composer.chat.mode === modelData
                    onTriggered: composer.chat.chooseMode(modelData)
                }
                onObjectAdded: (index, object) => modeMenu.insertItem(8 + index, object)
                onObjectRemoved: (index, object) => modeMenu.removeItem(object)
            }
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

    FileDialog {
        id: picker
        title: qsTr("Attach Files")
        fileMode: FileDialog.OpenFiles
        nameFilters: [qsTr("Text and pictures (*.txt *.md *.rs *.py *.js *.ts *.qml *.c *.cpp *.h *.json *.toml *.yaml *.yml *.csv *.log *.sh *.html *.css *.png *.jpg *.jpeg *.gif *.webp *.bmp)"), qsTr("All files (*)")]
        onAccepted: composer.chat.attachFiles(selectedFiles.map(u => decodeURIComponent(u.toString().replace(/^file:\/\//, ""))))
    }

    Timer {
        id: prepTimer
        interval: 400
        onTriggered: composer.chat.prepare(input.text)
    }
}
