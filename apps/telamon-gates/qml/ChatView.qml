pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import Telamon.Ui

// The open conversation: its messages, and the field to write the next one.
Item {
    id: view

    required property var chat

    // The column messages and the field share, centred in the view.
    readonly property real columnWidth: Math.min(width - Kirigami.Units.gridUnit * 3, Kirigami.Units.gridUnit * 46)

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
            Layout.topMargin: Kirigami.Units.largeSpacing * 2
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
        }

        InfoBanner {
            Layout.fillWidth: true
            Layout.maximumWidth: view.columnWidth
            Layout.alignment: Qt.AlignHCenter
            type: "info"
            shown: view.chat.demo
            closable: true
            text: qsTr("No model is connected yet: replies come from the built-in demo.")
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
                    enabled: view.chat.count > 0 && !view.chat.generating
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
                QQC2.ScrollBar.vertical: TelamonScrollBar {}

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

                delegate: MessageDelegate {
                    width: list.width
                    columnWidth: view.columnWidth
                    chat: view.chat
                    last: index === list.count - 1
                }
            }

            TelamonEmptyState {
                anchors.centerIn: parent
                width: Math.min(parent.width, Kirigami.Units.gridUnit * 24)
                visible: view.chat.count === 0 && !view.chat.loading
                symbol: Symbols.Forum
                title: qsTr("How Can I Help?")
                text: qsTr("Ask anything. Conversations are kept on this computer.")
            }

            TelamonSpinner {
                anchors.centerIn: parent
                visible: view.chat.loading
                animated: visible
            }
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
