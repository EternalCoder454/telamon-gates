pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import Telamon.Ui

// Above the composer in Agent mode: the folder the agent works in, a word
// when the model can't call tools, and the question the agent waits on
// before it changes a file or runs a command.
ColumnLayout {
    id: panel

    required property var chat

    // The model is one of ours and its template takes no tools (a server
    // elsewhere is not known: no warning).
    readonly property bool noTools: !panel.chat.demo && panel.chat.serverUrl.length === 0 && panel.chat.models.length > 0 && panel.chat.toolModels.indexOf(panel.chat.model) < 0

    spacing: TelamonStyle.spacing

    InfoBanner {
        Layout.fillWidth: true
        type: "warning"
        text: qsTr("%1 wasn't made to use tools, so the agent can only talk. Pick a model that can, such as Qwen3.").arg(panel.chat.model)
        shown: panel.noTools
    }

    // The question: what the agent wants to do, and the answers.
    Rectangle {
        visible: panel.chat.approving
        Layout.fillWidth: true
        implicitHeight: ask.implicitHeight + TelamonStyle.spacingLarge * 2
        radius: TelamonStyle.radiusLarge
        color: TelamonStyle.control
        border.width: 1
        border.color: TelamonStyle.accent

        ColumnLayout {
            id: ask
            anchors.fill: parent
            anchors.margins: TelamonStyle.spacingLarge
            spacing: TelamonStyle.spacing

            RowLayout {
                spacing: TelamonStyle.spacing

                Symbol {
                    icon: panel.chat.approvalKind === "run" ? Symbols.Terminal : Symbols.EditNote
                    color: TelamonStyle.accent
                }
                TelamonLabel {
                    Layout.fillWidth: true
                    textFormat: Text.PlainText
                    elide: Text.ElideMiddle
                    font.weight: Font.DemiBold
                    text: panel.chat.approvalTitle
                }
            }
            // The edit is for a file the user has changes of their own in,
            // not saved: they choose whose version stays.
            InfoBanner {
                Layout.fillWidth: true
                type: "warning"
                shown: panel.chat.approvalConflict
                text: qsTr("You have unsaved changes in this file. Keep Mine leaves the file and your changes as they are. Take the AI's replaces your changes with its version.")
            }
            // What the model wrote: plain text, never run or shown as HTML.
            TelamonCodeView {
                Layout.fillWidth: true
                text: panel.chat.approvalDetail
                maximumHeight: Kirigami.Units.gridUnit * 10
                // Wrapped: a long line can't hide its end off to the side.
                wrap: true
                Accessible.name: panel.chat.approvalKind === "run" ? qsTr("Command") : qsTr("Change")
            }
            // Right-aligned, and onto a second line when the chat is narrow
            // (beside the coding workspace); the order is reversed for that.
            Flow {
                Layout.fillWidth: true
                layoutDirection: Qt.RightToLeft
                spacing: TelamonStyle.spacing

                PrimaryButton {
                    text: panel.chat.approvalConflict ? qsTr("Take the AI's") : panel.chat.approvalKind === "run" ? qsTr("Run") : qsTr("Allow")
                    onClicked: panel.chat.answerApproval(1)
                }
                SecondaryButton {
                    visible: panel.chat.approvalKind === "write" && !panel.chat.approvalConflict
                    text: qsTr("Allow All Edits in This Reply")
                    onClicked: panel.chat.answerApproval(2)
                }
                SecondaryButton {
                    text: panel.chat.approvalConflict ? qsTr("Keep Mine") : qsTr("Deny")
                    onClicked: panel.chat.answerApproval(panel.chat.approvalConflict ? 3 : 0)
                }
            }
        }
    }

    // Where it works: its own sandbox, or a folder of the user's chosen on
    // purpose.
    RowLayout {
        Layout.fillWidth: true
        spacing: TelamonStyle.spacing

        Symbol {
            icon: panel.chat.sandboxed ? Symbols.Shield : Symbols.FolderOpen
            color: TelamonStyle.accent
        }
        TelamonLabel {
            Layout.fillWidth: true
            textFormat: Text.PlainText
            elide: Text.ElideMiddle
            wrapMode: panel.chat.sandboxed ? Text.Wrap : Text.NoWrap
            // Short when the chat is narrow (beside the coding workspace).
            text: panel.chat.sandboxed ? (panel.width < Kirigami.Units.gridUnit * 30 ? qsTr("Sandbox: a folder of its own") : qsTr("Sandbox: the agent works in a folder of its own and can't see the rest of your computer.")) : panel.chat.workspace
        }
        SecondaryButton {
            visible: panel.chat.sandboxed
            text: panel.width < Kirigami.Units.gridUnit * 30 ? qsTr("Use a Folder…") : qsTr("Use a Folder on This Computer…")
            enabled: !panel.chat.generating
            onClicked: realFolder.open()
        }
        SecondaryButton {
            visible: !panel.chat.sandboxed
            text: qsTr("Change…")
            enabled: !panel.chat.generating
            onClicked: panel.pickFolder()
        }
        SecondaryButton {
            visible: !panel.chat.sandboxed
            text: qsTr("Use the Sandbox")
            symbol: Symbols.Shield
            enabled: !panel.chat.generating
            onClicked: panel.chat.useSandbox()
        }
    }

    TelamonLabel {
        visible: !panel.chat.commandsAvailable
        Layout.fillWidth: true
        wrapMode: Text.Wrap
        textStyle: TelamonLabel.Caption
        opacity: 0.8
        text: qsTr("Commands are off: they run in a bubblewrap sandbox, and bubblewrap isn't installed.")
    }

    ConfirmDialog {
        id: realFolder
        title: qsTr("Let the Agent Into a Folder?")
        text: qsTr("The agent can read everything in the folder you choose, and changes files there or runs commands only after you allow it. Commands still run in a sandbox.")
        acceptText: qsTr("Choose Folder…")
        onAccepted: panel.pickFolder()
    }

    // The folder dialog is made the first time it is needed
    // (AgentFolderDialog.qml).
    Loader {
        id: pickerLoader
        readonly property var dialog: pickerLoader.item
        visible: false
    }

    function pickFolder() {
        if (!pickerLoader.item) {
            pickerLoader.setSource("AgentFolderDialog.qml", {
                chat: panel.chat
            });
        }
        pickerLoader.dialog.open();
    }
}
