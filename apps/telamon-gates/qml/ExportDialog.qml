import QtQuick.Dialogs

// Where to save the open conversation. A file of its own, loaded when Export
// is first used (ChatView.exportAs): QtQuick.Dialogs costs start-up time and
// memory that most sessions never need.
FileDialog {
    id: saver

    required property var chat
    property string format: "markdown"

    fileMode: FileDialog.SaveFile
    title: qsTr("Export Conversation")
    nameFilters: saver.format === "json" ? [qsTr("JSON (*.json)")] : [qsTr("Markdown (*.md)")]
    defaultSuffix: saver.format === "json" ? "json" : "md"
    selectedFile: "file:///" + encodeURIComponent((saver.chat.title.length > 0 ? saver.chat.title : qsTr("Conversation")).replace(/[\/:*?"<>|]/g, "-")) + (saver.format === "json" ? ".json" : ".md")
    onAccepted: saver.chat.exportTo(decodeURIComponent(selectedFile.toString().replace(/^file:\/\//, "")), saver.format)
}
