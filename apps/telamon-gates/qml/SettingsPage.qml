pragma ComponentBehavior: Bound

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
            visible: page.chat.models.length > 1
            title: qsTr("Model for Code")
            subtitle: qsTr("Used by Code and Agent modes")
            leading: [
                Symbol {
                    icon: Symbols.Code
                    color: TelamonStyle.accent
                }
            ]

            TelamonComboBox {
                // First "the same as Model", then every model.
                model: [qsTr("Same as Model")].concat(Array.from(page.chat.models))
                currentIndex: page.chat.codeModel.length > 0 ? Math.max(0, page.chat.models.indexOf(page.chat.codeModel) + 1) : 0
                onActivated: index => page.chat.pickCodeModel(index === 0 ? "" : page.chat.models[index - 1])
                Accessible.name: qsTr("Model for Code")
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
            visible: !page.chat.demo && page.chat.serverUrl.length === 0
            title: qsTr("Smaller Context Cache")
            subtitle: qsTr("About 30% less video memory for the same context, about 10% slower replies")
            leading: [
                Symbol {
                    icon: Symbols.Memory
                    color: TelamonStyle.accent
                }
            ]
            showSwitch: true
            switchChecked: page.chat.smallCache
            onSwitchToggled: checked => page.chat.useSmallCache(checked)
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
        title: qsTr("Modes")
        footer: qsTr("Each mode is a system prompt and a temperature. Change the built-in ones, or add your own: they show beside the mode switch under the message field.")

        Repeater {
            model: page.chat.modeIds

            SectionRow {
                id: modeRow
                required property int index
                required property string modelData
                readonly property bool own: index >= 5
                readonly property real temperature: page.chat.modeTemps[index] ?? -1
                readonly property string prompt: page.chat.modePrompts[index] ?? ""

                title: page.chat.modeNames[index] ?? ""
                // The prompt's start, so every row stays one line or two.
                readonly property string gist: modeRow.prompt.split("\n")[0]
                subtitle: (modeRow.prompt.length === 0 ? qsTr("No prompt of its own") : modeRow.gist.length > 70 ? modeRow.gist.slice(0, 70).trim() + "…" : modeRow.gist) + " · " + (modeRow.temperature < 0 ? qsTr("the model's temperature") : qsTr("temperature %1").arg(Number(modeRow.temperature).toLocaleString(Qt.locale(), "f", 2)))
                leading: [
                    Symbol {
                        icon: modeRow.modelData === "story" ? Symbols.AutoStories : modeRow.modelData === "code" ? Symbols.Code : modeRow.modelData === "agent" ? Symbols.SmartToy : modeRow.modelData === "research" ? Symbols.TravelExplore : modeRow.own ? Symbols.EditNote : Symbols.Chat
                        color: TelamonStyle.accent
                    }
                ]

                SecondaryButton {
                    text: qsTr("Edit…")
                    onClicked: modeDialog.edit(modeRow.modelData, modeRow.title, modeRow.prompt, modeRow.temperature, modeRow.own)
                }
            }
        }
        SectionRow {
            title: qsTr("Add a Mode")
            subtitle: qsTr("Your own prompt, such as a character to talk with or a house style")
            leading: [
                Symbol {
                    icon: Symbols.Add
                    color: TelamonStyle.accent
                }
            ]

            SecondaryButton {
                text: qsTr("Add…")
                onClicked: modeDialog.edit("", "", "", -1, true)
            }
        }
    }

    TelamonDialog {
        id: modeDialog

        property string modeId
        property bool own

        function edit(id, name, prompt, temperature, own) {
            modeDialog.modeId = id;
            modeDialog.own = own;
            nameField.text = name;
            promptArea.text = prompt;
            ownTemperature.checked = temperature < 0;
            temperatureSlider.value = temperature < 0 ? 0.7 : temperature;
            modeDialog.title = id.length === 0 ? qsTr("Add a Mode") : qsTr("Edit %1").arg(name);
            modeDialog.open();
        }

        footerContent: [
            SecondaryButton {
                visible: modeDialog.modeId.length > 0
                text: modeDialog.own ? qsTr("Delete") : qsTr("Reset")
                onClicked: {
                    page.chat.deleteMode(modeDialog.modeId);
                    modeDialog.close();
                }
            },
            SecondaryButton {
                text: qsTr("Cancel")
                onClicked: modeDialog.close()
            },
            PrimaryButton {
                text: qsTr("Save")
                enabled: !modeDialog.own || nameField.text.trim().length > 0
                onClicked: {
                    page.chat.saveMode(modeDialog.modeId, nameField.text, promptArea.text, ownTemperature.checked ? -1 : temperatureSlider.value);
                    modeDialog.close();
                }
            }
        ]

        TelamonLabel {
            text: qsTr("Name")
        }
        TelamonTextField {
            id: nameField
            Layout.fillWidth: true
            enabled: modeDialog.own
            placeholderText: qsTr("Such as Pirate or Tutor")
        }
        TelamonLabel {
            Layout.topMargin: TelamonStyle.spacing
            text: qsTr("System Prompt")
        }
        TelamonTextArea {
            id: promptArea
            Layout.fillWidth: true
            Layout.preferredHeight: Kirigami.Units.gridUnit * 8
            wrapMode: TextEdit.Wrap
            placeholderText: qsTr("How replies in this mode should be written")
        }
        RowLayout {
            Layout.topMargin: TelamonStyle.spacing
            Layout.fillWidth: true
            spacing: TelamonStyle.spacing

            TelamonLabel {
                Layout.fillWidth: true
                text: qsTr("The Model's Own Temperature")
            }
            TelamonSwitch {
                id: ownTemperature
                Accessible.name: qsTr("The Model's Own Temperature")
            }
        }
        RowLayout {
            Layout.fillWidth: true
            visible: !ownTemperature.checked
            spacing: TelamonStyle.spacing

            TelamonSlider {
                id: temperatureSlider
                Layout.fillWidth: true
                from: 0
                to: 2
                stepSize: 0.05
                Accessible.name: qsTr("Temperature")
            }
            TelamonLabel {
                Layout.preferredWidth: Kirigami.Units.gridUnit * 2
                horizontalAlignment: Text.AlignRight
                text: Number(temperatureSlider.value).toLocaleString(Qt.locale(), "f", 2)
            }
        }
        TelamonLabel {
            visible: !ownTemperature.checked
            Layout.fillWidth: true
            wrapMode: Text.Wrap
            textStyle: TelamonLabel.Caption
            opacity: 0.75
            text: qsTr("Lower is steadier and more exact; higher is freer and more surprising.")
        }
    }

    Section {
        title: qsTr("Agent")
        footer: qsTr("An agent's commands run in a bubblewrap sandbox: they see the agent's folder and the system's programs, nothing else of yours.")

        SectionRow {
            title: qsTr("Network for Commands")
            subtitle: qsTr("Let commands download, such as packages for a build")
            leading: [
                Symbol {
                    icon: Symbols.Public
                    color: TelamonStyle.accent
                }
            ]
            showSwitch: true
            switchChecked: page.chat.agentNetwork
            onSwitchToggled: checked => page.chat.setAgentAccess(checked, page.chat.agentHome)
        }
        SectionRow {
            title: qsTr("Your Tools in Commands")
            subtitle: qsTr("Show commands your home folder, read-only, for toolchains such as cargo or mise")
            leading: [
                Symbol {
                    icon: Symbols.Home
                    color: TelamonStyle.accent
                }
            ]
            showSwitch: true
            switchChecked: page.chat.agentHome
            onSwitchToggled: checked => page.chat.setAgentAccess(page.chat.agentNetwork, checked)
        }
    }

    Section {
        id: webSection
        title: qsTr("Web Search")
        footer: qsTr("Lets Chat, Code and Agent replies search the web and read pages, when the model can call tools. What the model asks for is sent to the search service. Pages are opened over https only, never on this computer or your network, and everything that comes back is treated as text to read, not orders to follow.")

        readonly property var providers: ["brave", "tavily", "searxng"]
        readonly property bool keyed: page.chat.webProvider !== "searxng"
        // Enough is set up to try a search.
        readonly property bool configured: keyed ? page.chat.webKeySaved && page.chat.keyringAvailable : page.chat.webUrl.length > 0

        SectionRow {
            title: qsTr("Web Search")
            subtitle: page.chat.webSearch && page.chat.webNote.length > 0 ? page.chat.webNote : qsTr("Let local models search the web and read pages")
            leading: [
                Symbol {
                    icon: Symbols.TravelExplore
                    color: TelamonStyle.accent
                }
            ]
            showSwitch: true
            switchChecked: page.chat.webSearch
            onSwitchToggled: checked => page.chat.enableWebSearch(checked)
        }
        SectionRow {
            visible: page.chat.webSearch
            title: qsTr("Search Service")
            subtitle: webSection.keyed ? qsTr("Needs an API key from the service") : qsTr("Your own SearXNG instance: no key")
            leading: [
                Symbol {
                    icon: Symbols.Search
                    color: TelamonStyle.accent
                }
            ]

            TelamonComboBox {
                model: [qsTr("Brave Search"), qsTr("Tavily"), qsTr("SearXNG")]
                currentIndex: Math.max(0, webSection.providers.indexOf(page.chat.webProvider))
                onActivated: index => page.chat.pickWebProvider(webSection.providers[index])
                Accessible.name: qsTr("Search Service")
            }
        }
        SectionRow {
            visible: page.chat.webSearch && webSection.keyed
            title: qsTr("API Key")
            subtitle: !page.chat.keyringAvailable ? qsTr("%1 The key can't be saved without it.").arg(page.chat.keyringNote) : page.chat.webKeySaved ? qsTr("Saved in the system keyring") : qsTr("Kept in the system keyring, never in a file")
            leading: [
                Symbol {
                    icon: page.chat.keyringAvailable ? Symbols.Key : Symbols.Lock
                    color: page.chat.keyringAvailable ? TelamonStyle.accent : TelamonStyle.error
                }
            ]

            TelamonPasswordField {
                id: keyField
                implicitWidth: Kirigami.Units.gridUnit * 12
                enabled: page.chat.keyringAvailable && !page.chat.webTesting
                placeholderText: page.chat.webKeySaved ? qsTr("Saved") : qsTr("Paste the key")
                onAccepted: {
                    page.chat.saveWebKey(keyField.text);
                    keyField.text = "";
                }
                Accessible.name: qsTr("API Key")
            }
            SecondaryButton {
                text: qsTr("Save")
                enabled: page.chat.keyringAvailable && !page.chat.webTesting && keyField.text.trim().length > 0
                onClicked: {
                    page.chat.saveWebKey(keyField.text);
                    keyField.text = "";
                }
            }
            SecondaryButton {
                visible: page.chat.webKeySaved
                text: qsTr("Remove")
                enabled: !page.chat.webTesting
                onClicked: page.chat.removeWebKey()
            }
        }
        SectionRow {
            visible: page.chat.webSearch && !webSection.keyed
            title: qsTr("Instance Address")
            subtitle: qsTr("Such as http://localhost:8080. The instance has to allow json under search.formats in its settings.yml.")
            leading: [
                Symbol {
                    icon: Symbols.Dns
                    color: TelamonStyle.accent
                }
            ]

            TelamonTextField {
                id: instanceField
                implicitWidth: Kirigami.Units.gridUnit * 14
                text: page.chat.webUrl
                placeholderText: qsTr("http://localhost:8080")
                inputMethodHints: Qt.ImhUrlCharactersOnly | Qt.ImhNoAutoUppercase
                readonly property bool valid: text.trim().length === 0 || /^https?:\/\/[^\s\/@]+(:\d+)?(\/[^\s?#]*)?$/.test(text.trim())
                errorText: valid ? "" : qsTr("Use an address like http://localhost:8080")
                onEditingFinished: {
                    if (valid && text.trim() !== page.chat.webUrl) {
                        page.chat.saveWebUrl(text.trim());
                    }
                }
                Accessible.name: qsTr("Instance Address")
            }
        }
        SectionRow {
            visible: page.chat.webSearch
            title: qsTr("Test Connection")
            subtitle: page.chat.webTestResult.length > 0 ? page.chat.webTestResult : qsTr("Tries one search with these settings")
            busy: page.chat.webTesting
            leading: [
                Symbol {
                    icon: page.chat.webTestResult.length === 0 ? Symbols.NetworkCheck : page.chat.webTestOk ? Symbols.CheckCircle : Symbols.Error
                    color: page.chat.webTestResult.length === 0 ? TelamonStyle.accent : page.chat.webTestOk ? TelamonStyle.success : TelamonStyle.error
                }
            ]

            SecondaryButton {
                visible: !page.chat.webTesting
                text: qsTr("Test Connection")
                enabled: webSection.configured
                onClicked: page.chat.testWebSearch()
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
