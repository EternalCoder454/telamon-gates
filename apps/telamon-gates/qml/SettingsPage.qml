import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import Telamon.Ui

TelamonPage {
    id: page

    required property var chat
    required property var library
    required property var models

    title: qsTr("Settings")

    // A local folder as a file:// URL, each part encoded: a # or % in the
    // path is a name, not URL syntax.
    function folderUrl(path) {
        return "file://" + path.split("/").map(encodeURIComponent).join("/");
    }

    // The context sizes on offer, in tokens; 0 is automatic.
    readonly property var contexts: [0, 4096, 8192, 16384, 32768, 65536, 131072]

    Section {
        title: qsTr("Model")

        SectionRow {
            title: qsTr("Backend")
            subtitle: page.chat.backendName
            leading: [
                Symbol {
                    icon: Symbols.Hub
                    color: TelamonStyle.accent
                }
            ]
        }
        SectionRow {
            visible: page.chat.models.length > 0
            title: qsTr("Model")
            subtitle: qsTr("Used for new replies")
            leading: [
                Symbol {
                    icon: Symbols.Psychology
                    color: TelamonStyle.accent
                }
            ]

            TelamonComboBox {
                model: page.chat.models
                currentIndex: page.chat.models.indexOf(page.chat.model)
                onActivated: index => page.chat.pickModel(page.chat.models[index])
                Accessible.name: qsTr("Model")
            }
        }
        SectionRow {
            visible: page.chat.modelsFolder.length > 0 && page.chat.serverUrl.length === 0
            title: qsTr("Models Folder")
            subtitle: page.chat.modelsFolder
            leading: [
                Symbol {
                    icon: Symbols.Inventory2
                    color: TelamonStyle.accent
                }
            ]

            SecondaryButton {
                text: qsTr("Open Folder")
                symbol: Symbols.FolderOpen
                onClicked: Qt.openUrlExternally(page.folderUrl(page.chat.modelsFolder))
            }
        }
    }

    Section {
        title: qsTr("Model Server")

        SectionRow {
            visible: !page.chat.demo && page.chat.serverUrl.length === 0
            title: qsTr("GPU Layers")
            subtitle: qsTr("Automatic fits the model to the graphics card's free memory")
            leading: [
                Symbol {
                    icon: Symbols.Memory
                    color: TelamonStyle.accent
                }
            ]

            TelamonSpinBox {
                id: gpuLayers
                from: 0
                to: 999
                editable: true
                value: page.chat.gpuLayers
                textFromValue: (value, locale) => value === 0 ? qsTr("Automatic") : Number(value).toLocaleString(locale, "f", 0)
                // A cleared or mistyped field keeps the value it had.
                valueFromText: (text, locale) => {
                    if (text === qsTr("Automatic")) {
                        return 0;
                    }
                    try {
                        return Number.fromLocaleString(locale, text);
                    } catch (e) {
                        return gpuLayers.value;
                    }
                }
                onValueModified: page.chat.saveServerOptions(value, page.chat.contextSize, page.chat.serverUrl)
                Accessible.name: qsTr("GPU Layers")
            }
        }
        SectionRow {
            visible: !page.chat.demo && page.chat.serverUrl.length === 0
            title: qsTr("Context Size")
            // What Automatic came to, once a reply has run.
            subtitle: page.chat.contextSize === 0 && page.chat.activeContext > 0 ? qsTr("How much of the conversation the model sees at once. Now %1 tokens.").arg(Number(page.chat.activeContext).toLocaleString(Qt.locale(), "f", 0)) : qsTr("How much of the conversation the model sees at once")
            leading: [
                Symbol {
                    icon: Symbols.Notes
                    color: TelamonStyle.accent
                }
            ]

            TelamonComboBox {
                model: page.contexts.map(n => n === 0 ? qsTr("Automatic") : qsTr("%1 tokens").arg(Number(n).toLocaleString(Qt.locale(), "f", 0)))
                currentIndex: Math.max(0, page.contexts.indexOf(page.chat.contextSize))
                onActivated: index => page.chat.saveServerOptions(page.chat.gpuLayers, page.contexts[index], page.chat.serverUrl)
                Accessible.name: qsTr("Context Size")
            }
        }
        SectionRow {
            title: qsTr("Server Address")
            subtitle: page.chat.demo ? qsTr("A llama.cpp server elsewhere, such as http://192.168.1.20:8080. Restart Telamon Gates to use it.") : qsTr("A llama.cpp server elsewhere, such as http://192.168.1.20:8080. Leave empty to run the model here.")
            leading: [
                Symbol {
                    icon: Symbols.Dns
                    color: TelamonStyle.accent
                }
            ]

            TelamonTextField {
                id: address
                implicitWidth: Kirigami.Units.gridUnit * 14
                text: page.chat.serverUrl
                placeholderText: qsTr("Run here")
                inputMethodHints: Qt.ImhUrlCharactersOnly | Qt.ImhNoAutoUppercase
                // Plain http only: the client has no TLS.
                readonly property bool valid: text.trim().length === 0 || /^http:\/\/[^\s\/]+(:\d+)?\/?$/.test(text.trim())
                errorText: valid ? "" : qsTr("Use an address like http://192.168.1.20:8080")
                onEditingFinished: {
                    if (valid && text.trim() !== page.chat.serverUrl) {
                        page.chat.saveServerOptions(page.chat.gpuLayers, page.chat.contextSize, text.trim());
                    }
                }
                Accessible.name: qsTr("Server Address")
            }
        }
    }

    Section {
        title: qsTr("SystemOne")
        footer: qsTr("In Auto, a small decision model reads each message and picks Chat, Story or Code before the chat model answers. When it isn't sure, Chat answers.")

        SectionRow {
            title: qsTr("SystemOne")
            subtitle: page.chat.systemOneReady ? qsTr("Picking with %1").arg(page.chat.decisionModel) : page.chat.systemOne ? qsTr("On, once a decision model is here") : qsTr("Off: Auto answers in Chat")
            leading: [
                Symbol {
                    icon: Symbols.Bolt
                    color: TelamonStyle.accent
                }
            ]
            showSwitch: true
            switchChecked: page.chat.systemOne
            onSwitchToggled: checked => page.chat.enableSystemOne(checked)
        }
        SectionRow {
            visible: page.chat.systemOne && page.chat.decisionModels.length > 1
            title: qsTr("Decision Model")
            subtitle: qsTr("Laya runs on the processor; Kev on the graphics card, faster and more accurate")
            leading: [
                Symbol {
                    icon: Symbols.Psychology
                    color: TelamonStyle.accent
                }
            ]

            TelamonComboBox {
                model: page.chat.decisionModels
                currentIndex: page.chat.decisionModels.indexOf(page.chat.decisionModel)
                onActivated: index => page.chat.pickDecisionModel(page.chat.decisionModels[index])
                Accessible.name: qsTr("Decision Model")
            }
        }
        // None yet: one click gets one, from its maker's official build.
        SectionRow {
            visible: page.chat.systemOne && page.chat.decisionModels.length === 0
            title: page.models.downloadRepo.length > 0 ? qsTr("Downloading %1…").arg(page.models.downloading) : qsTr("Get a Decision Model")
            subtitle: page.models.downloadRepo.length > 0 ? qsTr("%1% done. The Models page shows it too.").arg(Math.floor(page.models.progress * 100)) : qsTr("Laya: 429 MiB, on the processor. Kev: 2.8 GiB, on the graphics card.")
            busy: page.models.downloadRepo.length > 0
            leading: [
                Symbol {
                    icon: Symbols.Download
                    color: TelamonStyle.accent
                }
            ]

            SecondaryButton {
                visible: page.models.downloadRepo.length === 0
                text: qsTr("Get Laya")
                onClicked: page.models.downloadFrom("ggml-org/Laya-GGUF", "Laya-Q8_0.gguf")
            }
            SecondaryButton {
                visible: page.models.downloadRepo.length === 0
                text: qsTr("Get Kev")
                onClicked: page.models.downloadFrom("ggml-org/Kev-4B-GGUF", "Kev-4B-Q4_K_M.gguf")
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
            leading: [
                Symbol {
                    icon: Symbols.Folder
                    color: TelamonStyle.accent
                }
            ]

            SecondaryButton {
                text: qsTr("Open Folder")
                symbol: Symbols.FolderOpen
                // Each part encoded: a # or % in the path is a name, not URL syntax.
                onClicked: Qt.openUrlExternally(page.folderUrl(page.library.folder))
            }
        }
    }

    Component.onDestruction: {
        if (saveLater.running) {
            page.chat.saveSystemPrompt(prompt.text);
        }
    }
}
