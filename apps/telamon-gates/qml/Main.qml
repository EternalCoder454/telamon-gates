pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import Telamon.Ui

// The window: conversations in the sidebar, the open one beside it.
TelamonWindow {
    id: root

    // Set from main.cpp through setInitialProperties(); see src/lib.rs.
    required property var chat
    required property var library

    // "chat", "settings" or "about".
    property string page: "chat"

    title: root.chat.title.length > 0 ? qsTr("%1 — Telamon Gates").arg(root.chat.title) : qsTr("Telamon Gates")
    stateKey: "main"
    width: Kirigami.Units.gridUnit * 56
    height: Kirigami.Units.gridUnit * 40
    minimumWidth: Kirigami.Units.gridUnit * 24
    minimumHeight: Kirigami.Units.gridUnit * 20
    visible: true

    LayoutMirroring.enabled: Application.layoutDirection === Qt.RightToLeft
    LayoutMirroring.childrenInherit: true

    function newChat() {
        root.page = "chat";
        root.chat.newChat();
        chatView.focusComposer();
    }

    function openChat(id) {
        root.page = "chat";
        root.chat.open(id);
        chatView.focusComposer();
    }

    function confirmDelete(id, title) {
        root.confirm({
            title: qsTr("Delete Conversation?"),
            text: qsTr("“%1” will be deleted from this computer.").arg(title),
            acceptText: qsTr("Delete"),
            destructive: true
        }, ok => {
            if (ok) {
                root.library.remove(id);
            }
        });
    }

    // The sidebar's sections count back from the start of today, local time,
    // which only QML knows here. Checked each minute, for a window left open
    // past midnight.
    function updateDayStart() {
        const d = new Date();
        d.setHours(0, 0, 0, 0);
        root.library.setDayStart(d.getTime());
    }
    Timer {
        interval: 60 * 1000
        running: root.visible
        repeat: true
        triggeredOnStart: true
        onTriggered: root.updateDayStart()
    }

    // The list's rows by section, newest first in each.
    readonly property var bySection: {
        const m = {
            today: [],
            yesterday: [],
            week: [],
            month: [],
            older: []
        };
        const s = root.library.sections;
        for (let i = 0; i < s.length; ++i) {
            (m[s[i]] ?? m.older).push(i);
        }
        return m;
    }

    Shortcut {
        sequences: [StandardKey.New]
        onActivated: root.newChat()
    }

    // A section's name above its conversations; hidden while filtering and
    // when the sidebar is icons only.
    component NavHeading: QQC2.Label {
        required property var rows
        Layout.fillWidth: true
        Layout.topMargin: Kirigami.Units.largeSpacing
        Layout.bottomMargin: Kirigami.Units.smallSpacing
        Layout.leftMargin: Kirigami.Units.largeSpacing
        visible: rows.length > 0 && !sidebar.compact && sidebar.filterText.length === 0
        font: Kirigami.Theme.smallFont
        opacity: 0.6
        elide: Text.ElideRight
    }

    // One saved conversation; `modelData` is its row in the library.
    component ConversationItem: SidebarItem {
        required property int modelData
        readonly property string conversationId: root.library.ids[modelData] ?? ""
        Layout.fillWidth: true
        text: root.library.titles[modelData] ?? ""
        symbol: Symbols.ChatBubble
        selected: root.page === "chat" && root.chat.conversationId === conversationId
        onClicked: root.openChat(conversationId)
    }

    RowLayout {
        anchors.fill: parent
        spacing: 0

        TelamonSidebar {
            id: sidebar
            Layout.fillHeight: true
            Layout.preferredWidth: compact ? Kirigami.Units.gridUnit * 3.6 : Kirigami.Units.gridUnit * 14
            compact: root.sidebarCollapsed
            padding: Kirigami.Units.largeSpacing
            spacing: 2
            showFilter: !compact && root.library.ids.length > 0
            placeholderText: qsTr("No Matching Conversations")
            placeholderSymbol: Symbols.SearchOff

            Behavior on Layout.preferredWidth {
                NumberAnimation {
                    duration: TelamonStyle.durationShort
                    easing.type: Easing.OutCubic
                }
            }

            onContextMenuRequested: (item, pos) => {
                const conversation = item as ConversationItem;
                if (conversation && conversation.conversationId.length > 0) {
                    conversationMenu.conversationId = conversation.conversationId;
                    conversationMenu.conversationTitle = conversation.text;
                    conversationMenu.popup(sidebar, pos.x, pos.y);
                }
            }

            SidebarItem {
                Layout.fillWidth: true
                text: qsTr("New Chat")
                symbol: Symbols.EditSquare
                selected: root.page === "chat" && root.chat.conversationId.length === 0
                onClicked: root.newChat()
            }

            NavHeading {
                rows: root.bySection.today
                text: qsTr("Today")
            }
            Repeater {
                model: root.bySection.today
                ConversationItem {}
            }
            NavHeading {
                rows: root.bySection.yesterday
                text: qsTr("Yesterday")
            }
            Repeater {
                model: root.bySection.yesterday
                ConversationItem {}
            }
            NavHeading {
                rows: root.bySection.week
                text: qsTr("Previous 7 Days")
            }
            Repeater {
                model: root.bySection.week
                ConversationItem {}
            }
            NavHeading {
                rows: root.bySection.month
                text: qsTr("Previous 30 Days")
            }
            Repeater {
                model: root.bySection.month
                ConversationItem {}
            }
            NavHeading {
                rows: root.bySection.older
                text: qsTr("Older")
            }
            Repeater {
                model: root.bySection.older
                ConversationItem {}
            }

            footer: [
                SidebarItem {
                    Layout.fillWidth: true
                    text: qsTr("Settings")
                    symbol: Symbols.Settings
                    selected: root.page === "settings"
                    onClicked: root.page = "settings"
                },
                SidebarItem {
                    Layout.fillWidth: true
                    text: qsTr("About")
                    symbol: Symbols.Info
                    selected: root.page === "about"
                    onClicked: root.page = "about"
                }
            ]
        }

        Rectangle {
            Layout.fillHeight: true
            implicitWidth: 1
            color: Qt.alpha(Kirigami.Theme.textColor, 0.12)
        }

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            ChatView {
                id: chatView
                anchors.fill: parent
                visible: root.page === "chat"
                chat: root.chat
            }

            Loader {
                anchors.fill: parent
                active: root.page === "settings"
                visible: active
                sourceComponent: SettingsPage {
                    chat: root.chat
                    library: root.library
                }
            }

            Loader {
                anchors.fill: parent
                active: root.page === "about"
                visible: active
                sourceComponent: AboutPage {}
            }
        }
    }

    ContextMenu {
        id: conversationMenu
        property string conversationId
        property string conversationTitle

        ContextMenuItem {
            text: qsTr("Delete…")
            symbol: Symbols.Delete
            destructive: true
            onTriggered: root.confirmDelete(conversationMenu.conversationId, conversationMenu.conversationTitle)
        }
    }

    Component.onCompleted: chatView.focusComposer()
}
