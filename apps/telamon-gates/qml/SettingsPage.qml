import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import Telamon.Ui

TelamonPage {
    id: page

    required property var chat
    required property var library

    title: qsTr("Settings")

    Section {
        title: qsTr("Model")

        SectionRow {
            title: qsTr("Backend")
            subtitle: page.chat.backendName
        }
        SectionRow {
            visible: page.chat.models.length > 0
            title: qsTr("Model")
            subtitle: qsTr("Used for new replies")

            TelamonComboBox {
                model: page.chat.models
                currentIndex: page.chat.models.indexOf(page.chat.model)
                onActivated: index => page.chat.pickModel(page.chat.models[index])
                Accessible.name: qsTr("Model")
            }
        }
    }

    Section {
        title: qsTr("System Prompt")

        ColumnLayout {
            Layout.fillWidth: true
            Layout.margins: Kirigami.Units.largeSpacing
            spacing: Kirigami.Units.smallSpacing

            TelamonTextArea {
                id: prompt
                Layout.fillWidth: true
                implicitHeight: Kirigami.Units.gridUnit * 6
                wrapMode: TextEdit.Wrap
                placeholderText: qsTr("You are a helpful assistant.")
                text: page.chat.systemPrompt
                Accessible.name: qsTr("System Prompt")
                onTextChanged: saveLater.restart()
            }
            TelamonLabel {
                Layout.fillWidth: true
                textStyle: TelamonLabel.Caption
                wrapMode: Text.Wrap
                text: qsTr("Sent to the model before every conversation. Leave it empty for none.")
            }
        }

        // Saved once typing pauses, and when the page closes.
        Timer {
            id: saveLater
            interval: 600
            onTriggered: page.chat.saveSystemPrompt(prompt.text)
        }
    }

    Section {
        title: qsTr("Appearance")
        TelamonTransparencySwitch {}
    }

    Section {
        title: qsTr("Conversations")

        SectionRow {
            title: qsTr("Folder")
            subtitle: page.library.folder

            SecondaryButton {
                text: qsTr("Open Folder")
                symbol: Symbols.FolderOpen
                onClicked: Qt.openUrlExternally("file://" + page.library.folder)
            }
        }
    }

    Component.onDestruction: {
        if (saveLater.running) {
            page.chat.saveSystemPrompt(prompt.text);
        }
    }
}
