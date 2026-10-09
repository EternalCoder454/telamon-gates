import QtQuick.Dialogs

// The folder Agent mode works in. A file of its own, loaded when first used
// (AgentPanel.pickFolder); see ExportDialog.
FolderDialog {
    required property var chat

    title: qsTr("Folder for the Agent")
    onAccepted: chat.chooseWorkspace(decodeURIComponent(selectedFolder.toString().replace(/^file:\/\//, "")))
}
