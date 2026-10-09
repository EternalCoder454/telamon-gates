pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Dialogs
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import Telamon.Ui

// The open conversation: its messages, and the field to write the next one.
Item {
    id: view

    required property var chat

    // The column messages and the field share, centred in the view.
    readonly property real columnWidth: Math.min(width - Kirigami.Units.gridUnit * 3, Kirigami.Units.gridUnit * 46)

    // An empty chat greets the user by the time of day: morning from 5,
    // afternoon from noon, evening from 5 pm, night from 8 pm. Each has a
    // few ways of saying it; one is picked for every new chat.
    property int hour: new Date().getHours()
    property int pick: Math.floor(Math.random() * 1000)
    readonly property string period: hour >= 5 && hour < 12 ? "morning" : hour >= 12 && hour < 17 ? "afternoon" : hour >= 17 && hour < 20 ? "evening" : "night"
    readonly property string name: view.chat.userName.length > 0 ? view.chat.userName : qsTr("friend")
    readonly property var greetings: ({
            "morning": [qsTr("Good morning, %1"), qsTr("Morning, %1. Coffee first?"), qsTr("Rise and shine, %1"), qsTr("A fresh start, %1"), qsTr("Top of the morning, %1")],
            "afternoon": [qsTr("Good afternoon, %1"), qsTr("Afternoon, %1. What's next?"), qsTr("Hey %1, how's the day going?"), qsTr("Back at it, %1?")],
            "evening": [qsTr("Good evening, %1"), qsTr("Evening, %1. Winding down?"), qsTr("Hello again, %1"), qsTr("The day's almost done, %1")],
            "night": [qsTr("Cool midnight, %1"), qsTr("Burning the midnight oil, %1?"), qsTr("Quiet hours, %1"), qsTr("Night owl mode, %1"), qsTr("Still up, %1?")]
        })
    readonly property string greeting: {
        const list = view.greetings[view.period];
        return list[view.pick % list.length].arg(view.name);
    }
    readonly property string greetingText: {
        switch (view.period) {
        case "morning":
            return qsTr("What's first on today's list?");
        case "afternoon":
            return qsTr("What can I help you get done?");
        case "evening":
            return qsTr("Anything to wrap up before the day ends?");
        }
        return qsTr("Everything's quiet. What's on your mind?");
    }
    readonly property int greetingSymbol: view.period === "morning" ? Symbols.WbSunny : view.period === "afternoon" ? Symbols.LightMode : view.period === "evening" ? Symbols.WbTwilight : Symbols.Bedtime

    function regreet() {
        view.hour = new Date().getHours();
        view.pick = Math.floor(Math.random() * 1000);
    }

    // The hour moves on while an empty chat waits.
    Timer {
        interval: 60 * 1000
        running: view.visible && view.chat.count === 0
        repeat: true
        onTriggered: view.hour = new Date().getHours()
    }
    onPeriodChanged: view.pick = Math.floor(Math.random() * 1000)

    Connections {
        target: view.chat
        function onConversationIdChanged() {
            if (view.chat.conversationId.length === 0) {
                view.regreet();
            }
        }
    }

    function focusComposer() {
        composer.focusInput();
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // The conversation's title, and the model when there is a choice.
        RowLayout {
            Layout.fillWidth: true
            Layout.maximumWidth: view.columnWidth
            Layout.alignment: Qt.AlignHCenter
            // Level with TelamonPage's titles (Settings, About).
            Layout.topMargin: TelamonStyle.spacingXLarge + TelamonStyle.spacingLarge
            Layout.bottomMargin: Kirigami.Units.largeSpacing
            spacing: Kirigami.Units.largeSpacing

            TelamonLabel {
                Layout.fillWidth: true
                textStyle: TelamonLabel.Title
                textFormat: Text.PlainText
                elide: Text.ElideRight
                text: view.chat.title.length > 0 ? view.chat.title : qsTr("New Chat")
            }
            TelamonComboBox {
                visible: view.chat.models.length > 1
                model: view.chat.models
                currentIndex: view.chat.models.indexOf(view.chat.model)
                onActivated: index => view.chat.pickModel(view.chat.models[index])
                Accessible.name: qsTr("Model")
            }
            // Save the conversation as a file: Markdown to read, or JSON.
            ToolbarButton {
                visible: view.chat.count > 0
                symbol: Symbols.Download
                text: qsTr("Export…")
                focusable: true
                onClicked: exportMenu.popup()

                ContextMenu {
                    id: exportMenu
                    ContextMenuItem {
                        text: qsTr("Markdown…")
                        onTriggered: {
                            saver.format = "markdown";
                            saver.open();
                        }
                    }
                    ContextMenuItem {
                        text: qsTr("JSON…")
                        onTriggered: {
                            saver.format = "json";
                            saver.open();
                        }
                    }
                }
            }
        }

        FileDialog {
            id: saver
            property string format: "markdown"
            fileMode: FileDialog.SaveFile
            title: qsTr("Export Conversation")
            nameFilters: saver.format === "json" ? [qsTr("JSON (*.json)")] : [qsTr("Markdown (*.md)")]
            defaultSuffix: saver.format === "json" ? "json" : "md"
            selectedFile: "file:///" + encodeURIComponent((view.chat.title.length > 0 ? view.chat.title : qsTr("Conversation")).replace(/[\/:*?"<>|]/g, "-")) + (saver.format === "json" ? ".json" : ".md")
            onAccepted: view.chat.exportTo(decodeURIComponent(selectedFile.toString().replace(/^file:\/\//, "")), saver.format)
        }

        InfoBanner {
            Layout.fillWidth: true
            Layout.maximumWidth: view.columnWidth
            Layout.alignment: Qt.AlignHCenter
            type: "info"
            shown: view.chat.demo
            closable: true
            text: qsTr("No model server is installed: replies come from the built-in demo. Install telamon-llama to chat with local models.")
        }

        // The model server is there, but no model yet.
        InfoBanner {
            Layout.fillWidth: true
            Layout.maximumWidth: view.columnWidth
            Layout.alignment: Qt.AlignHCenter
            type: "info"
            shown: !view.chat.demo && view.chat.serverUrl.length === 0 && view.chat.models.length === 0 && view.chat.modelsFolder.length > 0
            text: qsTr("No model yet: add a .gguf model file to the models folder.")
            actions: [
                QQC2.Action {
                    text: qsTr("Open Folder")
                    onTriggered: Qt.openUrlExternally("file://" + view.chat.modelsFolder.split("/").map(encodeURIComponent).join("/"))
                }
            ]
        }

        // A model dropped into the folder shows up when the window is back.
        Connections {
            target: Application
            function onStateChanged() {
                if (Application.state === Qt.ApplicationActive && !view.chat.demo && view.chat.serverUrl.length === 0) {
                    view.chat.refreshModels();
                }
            }
        }

        InfoBanner {
            Layout.fillWidth: true
            Layout.maximumWidth: view.columnWidth
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: Kirigami.Units.smallSpacing
            type: "error"
            shown: view.chat.error.length > 0
            closable: true
            text: view.chat.error
            onClosed: view.chat.dismissError()
            actions: [
                QQC2.Action {
                    text: qsTr("Try Again")
                    // Only when the last reply failed or never came: a
                    // good one is never thrown away from here.
                    enabled: view.chat.retryable && !view.chat.generating
                    onTriggered: view.chat.regenerate()
                }
            ]
        }

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            ListView {
                id: list
                anchors.fill: parent
                model: view.chat
                clip: true
                spacing: Kirigami.Units.largeSpacing * 2
                topMargin: Kirigami.Units.largeSpacing
                bottomMargin: Kirigami.Units.largeSpacing * 2
                boundsBehavior: Flickable.StopAtBounds
                // Each message is laid out once and kept: replies are long.
                reuseItems: false
                QQC2.ScrollBar.vertical: TelamonScrollBar {
                    // Dragging the bar during a reply stops following it.
                    onPressedChanged: list.follow = pressed ? false : list.atYEnd
                }

                // Follows a growing reply while the view is at the end; a
                // scroll away from it stops that until the end is reached
                // again.
                property bool follow: true
                property bool pinning: false

                function pin() {
                    pinning = true;
                    positionViewAtEnd();
                    pinning = false;
                }

                onContentYChanged: {
                    if (!pinning && (moving || dragging)) {
                        follow = atYEnd;
                    }
                }
                onMovementEnded: follow = atYEnd
                onContentHeightChanged: {
                    if (follow) {
                        Qt.callLater(pin);
                    }
                }
                onCountChanged: {
                    follow = true;
                    Qt.callLater(pin);
                }

                // Another conversation opens at its end, wherever this one was.
                Connections {
                    target: view.chat
                    function onConversationIdChanged() {
                        list.follow = true;
                        Qt.callLater(list.pin);
                    }
                    function onLoadingChanged() {
                        list.follow = true;
                        Qt.callLater(list.pin);
                    }
                }


                delegate: MessageDelegate {
                    width: list.width
                    columnWidth: view.columnWidth
                    chat: view.chat
                    last: index === list.count - 1
                }
            }

            // The greeting: the hour's symbol in a soft accent circle, then
            // the words.
            ColumnLayout {
                anchors.centerIn: parent
                width: Math.min(parent.width - Kirigami.Units.gridUnit * 2, Kirigami.Units.gridUnit * 28)
                visible: view.chat.count === 0 && !view.chat.loading
                spacing: TelamonStyle.spacing

                Rectangle {
                    Layout.alignment: Qt.AlignHCenter
                    Layout.bottomMargin: TelamonStyle.spacingSmall
                    implicitWidth: Kirigami.Units.gridUnit * 3.5
                    implicitHeight: implicitWidth
                    radius: width / 2
                    color: Qt.alpha(TelamonStyle.accent, 0.14)

                    Symbol {
                        anchors.centerIn: parent
                        icon: view.greetingSymbol
                        size: Kirigami.Units.gridUnit * 2
                        color: TelamonStyle.accent
                    }
                }
                TelamonLabel {
                    Layout.fillWidth: true
                    textStyle: TelamonLabel.Title
                    textFormat: Text.PlainText
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    text: view.greeting
                }
                TelamonLabel {
                    Layout.fillWidth: true
                    textFormat: Text.PlainText
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    opacity: 0.7
                    text: view.greetingText
                }
            }

            // Messages fade out under the banner and the field rather than
            // being cut off at a hard edge.
            Rectangle {
                anchors.top: list.top
                anchors.left: list.left
                anchors.right: list.right
                height: Kirigami.Units.gridUnit
                visible: !list.atYBeginning
                gradient: Gradient {
                    GradientStop {
                        position: 0
                        color: Kirigami.Theme.backgroundColor
                    }
                    GradientStop {
                        position: 1
                        color: Qt.alpha(Kirigami.Theme.backgroundColor, 0)
                    }
                }
            }
            Rectangle {
                anchors.bottom: list.bottom
                anchors.left: list.left
                anchors.right: list.right
                height: Kirigami.Units.gridUnit
                visible: !list.atYEnd
                gradient: Gradient {
                    GradientStop {
                        position: 0
                        color: Qt.alpha(Kirigami.Theme.backgroundColor, 0)
                    }
                    GradientStop {
                        position: 1
                        color: Kirigami.Theme.backgroundColor
                    }
                }
            }

            TelamonSpinner {
                anchors.centerIn: parent
                visible: view.chat.loading
                animated: visible
            }
        }

        AgentPanel {
            visible: view.chat.mode === "agent"
            Layout.fillWidth: true
            Layout.maximumWidth: view.columnWidth
            Layout.alignment: Qt.AlignHCenter
            chat: view.chat
        }

        Composer {
            id: composer
            Layout.fillWidth: true
            Layout.maximumWidth: view.columnWidth
            Layout.alignment: Qt.AlignHCenter
            Layout.bottomMargin: Kirigami.Units.largeSpacing * 2
            chat: view.chat
            onSent: {
                list.follow = true;
                Qt.callLater(list.pin);
            }
        }
    }
}
