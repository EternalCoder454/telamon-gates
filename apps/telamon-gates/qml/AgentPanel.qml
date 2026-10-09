pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Dialogs
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
    readonly property bool noTools: panel.chat.serverUrl.length === 0 && panel.chat.models.length > 0 && panel.chat.toolModels.indexOf(panel.chat.model) < 0

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
            // What the model wrote: plain text, never run or shown as HTML.
            TelamonCodeView {
                Layout.fillWidth: true
                text: panel.chat.approvalDetail
                maximumHeight: Kirigami.Units.gridUnit * 10
                // Wrapped: a long line can't hide its end off to the side.
                wrap: true
                Accessible.name: panel.chat.approvalKind === "run" ? qsTr("Command") : qsTr("Change")
            }
            RowLayout {
                Layout.alignment: Qt.AlignRight
                spacing: TelamonStyle.spacing

                SecondaryButton {
                    text: qsTr("Deny")
                    onClicked: panel.chat.answerApproval(0)
                }
                SecondaryButton {
                    visible: panel.chat.approvalKind === "write"
                    text: qsTr("Allow All Edits in This Reply")
                    onClicked: panel.chat.answerApproval(2)
                }
                PrimaryButton {
                    text: panel.chat.approvalKind === "run" ? qsTr("Run") : qsTr("Allow")
                    onClicked: panel.chat.answerApproval(1)
                }
            }
        }
    }

    // The folder.
    RowLayout {
        Layout.fillWidth: true
        spacing: TelamonStyle.spacing

        Symbol {
            icon: Symbols.FolderOpen
            color: TelamonStyle.accent
        }
        TelamonLabel {
            Layout.fillWidth: true
            textFormat: Text.PlainText
            elide: Text.ElideMiddle
            opacity: panel.chat.workspace.length > 0 ? 1 : 0.7
            text: panel.chat.workspace.length > 0 ? panel.chat.workspace : qsTr("Choose the folder the agent works in. It can read anything there, and asks before it changes a file or runs a command.")
            wrapMode: panel.chat.workspace.length > 0 ? Text.NoWrap : Text.Wrap
        }
        SecondaryButton {
            text: panel.chat.workspace.length > 0 ? qsTr("Change…") : qsTr("Choose Folder…")
            enabled: !panel.chat.generating
            symbol: Symbols.FolderOpen
            onClicked: picker.open()
        }
    }

    FolderDialog {
        id: picker
        title: qsTr("Folder for the Agent")
        onAccepted: panel.chat.chooseWorkspace(decodeURIComponent(selectedFolder.toString().replace(/^file:\/\//, "")))
    }
}
