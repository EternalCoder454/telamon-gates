import QtQuick.Dialogs

// Which files to send with the next message. A file of its own, loaded when
// Attach is first used (Composer.pick); see ExportDialog.
FileDialog {
    required property var chat

    title: qsTr("Attach Files")
    fileMode: FileDialog.OpenFiles
    nameFilters: [qsTr("Text and pictures (*.txt *.md *.rs *.py *.js *.ts *.qml *.c *.cpp *.h *.json *.toml *.yaml *.yml *.csv *.log *.sh *.html *.css *.png *.jpg *.jpeg *.gif *.webp *.bmp)"), qsTr("All files (*)")]
    onAccepted: chat.attachFiles(selectedFiles.map(u => decodeURIComponent(u.toString().replace(/^file:\/\//, ""))))
}
