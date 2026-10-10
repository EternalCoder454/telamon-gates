pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import QtQml.Models
import org.kde.kirigami as Kirigami
import Telamon.Ui

// The window: conversations in the sidebar, the open one beside it.
TelamonWindow {
    id: root

    // Set from main.cpp through setInitialProperties(); see src/lib.rs.
    required property var chat
    required property var library
    required property var vram
    required property var models
    required property var fleet
    required property var workbench

    // "chat", "fleet", "models", "settings" or "about".
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

    // The workspace panel, when it is open (made when first opened).
    readonly property var workspacePanel: workspaceLoader.item

    // Runs `proceed` unless the workspace has files with unsaved changes:
    // then the user is asked first, as leaving would lose them.
    function unlessUnsaved(proceed) {
        if (root.workspacePanel === null || !root.workspacePanel.hasUnsaved) {
            proceed();
            return;
        }
        root.confirm({
            title: qsTr("Discard Unsaved Changes?"),
            text: qsTr("Files in the workspace have changes that are not saved. They will be lost."),
            acceptText: qsTr("Discard"),
            destructive: true
        }, ok => {
            if (ok) {
                proceed();
            }
        });
    }

    function newChat() {
        root.unlessUnsaved(() => {
            root.page = "chat";
            root.chat.newChat();
            chatView.focusComposer();
        });
    }

    function openChat(id) {
        root.unlessUnsaved(() => {
            root.page = "chat";
            root.chat.open(id);
            chatView.focusComposer();
        });
    }

    // Opens or closes the workspace panel of the open conversation.
    function showWorkspace(open) {
        if (open) {
            root.chat.showWorkspacePanel(true);
        } else {
            root.unlessUnsaved(() => root.chat.showWorkspacePanel(false));
        }
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

    // What the sidebar lists under New Chat, in order: a heading where the
    // section changes, then that section's conversations. One flat list,
    // since TelamonSidebar looks for its entries among its own children.
    ListModel {
        id: entries
    }

    // A row's heading: Today, Yesterday and Previous 7 Days, then its date
    // up to 30 days back, then its month.
    function heading(i) {
        const when = new Date(root.library.updates[i] ?? 0);
        switch (root.library.sections[i]) {
        case "today":
            return qsTr("Today");
        case "yesterday":
            return qsTr("Yesterday");
        case "week":
            return qsTr("Previous 7 Days");
        case "day":
            return Qt.locale().toString(when, "MMMM d, yyyy");
        }
        return Qt.locale().toString(when, "MMMM yyyy");
    }

    function rebuildEntries() {
        entries.clear();
        let last = "";
        for (let i = 0; i < root.library.ids.length; ++i) {
            const h = root.heading(i);
            if (h !== last) {
                entries.append({
                    kind: "heading",
                    label: h,
                    row: -1
                });
                last = h;
            }
            entries.append({
                kind: "conversation",
                label: "",
                row: i
            });
        }
    }

    Connections {
        target: root.library
        function onIdsChanged() {
            Qt.callLater(root.rebuildEntries);
        }
        function onSectionsChanged() {
            Qt.callLater(root.rebuildEntries);
        }
        function onUpdatesChanged() {
            Qt.callLater(root.rebuildEntries);
        }
    }

    // The graphics card's memory, read every few seconds while the window
    // can be seen.
    Timer {
        interval: 3000
        running: root.visible && root.visibility !== Window.Minimized
        repeat: true
        triggeredOnStart: true
        onTriggered: root.vram.refresh()
    }

    function gib(bytes, decimals) {
        return Qt.locale().toString(bytes / 1073741824, "f", decimals);
    }

    Shortcut {
        sequences: [StandardKey.New]
        onActivated: root.newChat()
    }

    // A heading above its conversations, level with their titles; hidden
    // while filtering and when the sidebar is icons only.
    component NavHeading: QQC2.Label {
        Layout.fillWidth: true
        Layout.topMargin: TelamonStyle.spacing
        Layout.bottomMargin: 0
        // Where a SidebarItem's title starts: past its icon slot.
        Layout.leftMargin: TelamonStyle.spacingLarge * 2 + Kirigami.Units.iconSizes.smallMedium
        visible: !sidebar.compact && sidebar.filterText.length === 0
        font: Kirigami.Theme.smallFont
        opacity: 0.72
        elide: Text.ElideRight
        textFormat: Text.PlainText
    }

    // One saved conversation; `row` is its row in the library. Denser than
    // the pages and without an icon: there are many, all alike, and the
    // heading above says what they are. The title lines up with the pages'.
    component ConversationItem: SidebarItem {
        required property int row
        readonly property string conversationId: root.library.ids[row] ?? ""
        Layout.fillWidth: true
        density: TelamonStyle.Compact
        text: root.library.titles[row] ?? ""
        selected: root.page === "chat" && root.chat.conversationId === conversationId
        onClicked: root.openChat(conversationId)
    }

    // How much of the graphics card's memory is in use: what a local model
    // has room for. Hidden when no card reports it, and when icons only.
    component VramMeter: ColumnLayout {
        id: meter
        readonly property real fraction: root.vram.total > 0 ? root.vram.used / root.vram.total : 0

        Layout.fillWidth: true
        // As a SidebarItem lays out its icon and title, so the meter lines
        // up with Settings and About under it.
        Layout.leftMargin: TelamonStyle.spacingLarge
        Layout.rightMargin: TelamonStyle.spacingLarge
        Layout.topMargin: TelamonStyle.spacingSmall
        Layout.bottomMargin: TelamonStyle.spacing
        visible: root.vram.available && !sidebar.compact
        spacing: TelamonStyle.spacingSmall
        Accessible.role: Accessible.ProgressBar
        Accessible.name: qsTr("Video memory: %1 of %2 GiB used").arg(root.gib(root.vram.used, 1)).arg(root.gib(root.vram.total, 0))

        RowLayout {
            Layout.fillWidth: true
            spacing: TelamonStyle.spacingLarge

            Item {
                Layout.preferredWidth: Kirigami.Units.iconSizes.smallMedium
                Layout.preferredHeight: Kirigami.Units.iconSizes.smallMedium

                Symbol {
                    anchors.centerIn: parent
                    icon: Symbols.Memory
                    size: Math.round(Kirigami.Units.iconSizes.smallMedium * 1.2)
                    color: TelamonStyle.accent
                }
            }
            QQC2.Label {
                Layout.fillWidth: true
                text: qsTr("VRAM")
                elide: Text.ElideRight
            }
            QQC2.Label {
                text: qsTr("%1 / %2 GiB").arg(root.gib(root.vram.used, 1)).arg(root.gib(root.vram.total, 0))
                font: Kirigami.Theme.smallFont
                opacity: 0.7
            }
        }
        UsageBar {
            Layout.fillWidth: true
            total: root.vram.total
            values: [root.vram.used]
            legend: false
            colors: [meter.fraction > 0.9 ? TelamonStyle.error : TelamonStyle.accent]
        }
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
            // Always there, so New Chat doesn't move when the first
            // conversation arrives.
            showFilter: !compact
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
                    conversationMenu.show(sidebar, pos.x, pos.y);
                }
            }

            SidebarItem {
                Layout.fillWidth: true
                text: qsTr("New Chat")
                symbol: Symbols.EditSquare
                selected: root.page === "chat" && root.chat.conversationId.length === 0
                onClicked: root.newChat()
            }

            // Icons only, a column of identical bubbles would say nothing:
            // the conversations show when the sidebar has room for titles.
            // Before the first conversation, what will be here.
            QQC2.Label {
                Layout.fillWidth: true
                Layout.topMargin: TelamonStyle.spacing
                Layout.leftMargin: TelamonStyle.spacingLarge * 2 + Kirigami.Units.iconSizes.smallMedium
                Layout.rightMargin: TelamonStyle.spacingLarge
                visible: root.library.loaded && root.library.ids.length === 0 && !sidebar.compact
                wrapMode: Text.Wrap
                font: Kirigami.Theme.smallFont
                opacity: 0.72
                text: qsTr("No conversations yet.")
            }

            Repeater {
                model: sidebar.compact ? null : entries
                delegate: DelegateChooser {
                    role: "kind"
                    DelegateChoice {
                        roleValue: "heading"
                        NavHeading {
                            required property string label
                            text: label
                        }
                    }
                    DelegateChoice {
                        roleValue: "conversation"
                        ConversationItem {}
                    }
                }
            }

            footer: [
                VramMeter {},
                SidebarItem {
                    Layout.fillWidth: true
                    text: qsTr("Fleet")
                    symbol: Symbols.Hub
                    selected: root.page === "fleet"
                    onClicked: root.page = "fleet"
                },
                SidebarItem {
                    Layout.fillWidth: true
                    text: qsTr("Models")
                    symbol: Symbols.Psychology
                    selected: root.page === "models"
                    onClicked: root.page = "models"
                },
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
            id: content
            Layout.fillWidth: true
            Layout.fillHeight: true

            // The coding workspace beside the chat (Code and Agent mode).
            // Both fit side by side from this width; narrower, the panel
            // takes the whole view and its Close button brings the chat back.
            readonly property bool workspaceShown: root.page === "chat" && root.chat.workspaceOpen && (root.chat.mode === "code" || root.chat.mode === "agent")
            readonly property bool sideBySide: content.width >= Kirigami.Units.gridUnit * 36
            readonly property real workspaceMinimum: Kirigami.Units.gridUnit * 20
            readonly property real workspaceMaximum: Math.max(content.workspaceMinimum, content.width - Kirigami.Units.gridUnit * 16)
            // How wide the user has dragged it (half the view to begin with).
            property real workspaceWidth: 0
            readonly property real workspaceShownWidth: content.sideBySide ? Math.max(content.workspaceMinimum, Math.min(content.workspaceMaximum, content.workspaceWidth > 0 ? content.workspaceWidth : content.width * 0.5)) : content.width

            ChatView {
                id: chatView
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                anchors.left: parent.left
                anchors.right: content.workspaceShown && content.sideBySide ? workspaceLoader.left : parent.right
                visible: root.page === "chat" && !(content.workspaceShown && !content.sideBySide)
                chat: root.chat
                library: root.library
                onWorkspaceRequested: open => root.showWorkspace(open)
            }

            // The panel is made when first opened, and let go when closed.
            Loader {
                id: workspaceLoader
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                anchors.right: parent.right
                width: content.workspaceShownWidth
                active: false
                visible: active

                function sync() {
                    if (content.workspaceShown) {
                        workspaceLoader.setSource("WorkspacePanel.qml", {
                            chat: root.chat,
                            workbench: root.workbench
                        });
                        workspaceLoader.active = true;
                    } else {
                        workspaceLoader.active = false;
                    }
                }
                Connections {
                    target: content
                    function onWorkspaceShownChanged() {
                        workspaceLoader.sync();
                    }
                }
                Component.onCompleted: workspaceLoader.sync()

                Connections {
                    target: workspaceLoader.item
                    ignoreUnknownSignals: true
                    function onCloseRequested() {
                        root.showWorkspace(false);
                    }
                }

                // The line between the chat and the panel, to drag.
                Rectangle {
                    anchors.top: parent.top
                    anchors.bottom: parent.bottom
                    anchors.left: parent.left
                    width: 1
                    visible: content.sideBySide
                    color: Qt.alpha(Kirigami.Theme.textColor, 0.12)
                }
                MouseArea {
                    z: 10
                    anchors.top: parent.top
                    anchors.bottom: parent.bottom
                    anchors.horizontalCenter: parent.left
                    width: Kirigami.Units.largeSpacing
                    visible: content.sideBySide
                    cursorShape: Qt.SplitHCursor
                    onPositionChanged: mouse => {
                        if (pressed) {
                            const x = mapToItem(content, mouse.x, 0).x;
                            content.workspaceWidth = Math.max(content.workspaceMinimum, Math.min(content.workspaceMaximum, content.width - x));
                        }
                    }
                }
            }

            // The other pages, one at a time, made when shown and dropped
            // when left. Loaded by file name, not by type: a type named here
            // would be loaded with the window, along with everything it
            // imports (about 15 ms of the start for these four).
            Loader {
                id: pageLoader
                anchors.fill: parent
                active: false
                visible: active
            }
        }
    }

    // The pages' files and what each is given: only the one shown exists.
    readonly property var lazyPages: ({
            "settings": {
                url: "SettingsPage.qml",
                props: {
                    chat: root.chat,
                    library: root.library,
                    models: root.models
                }
            },
            "fleet": {
                url: "FleetPage.qml",
                props: {
                    fleet: root.fleet,
                    chat: root.chat
                }
            },
            "models": {
                url: "ModelsPage.qml",
                props: {
                    models: root.models,
                    vram: root.vram,
                    chat: root.chat,
                    confirm: root.confirm
                }
            },
            "about": {
                url: "AboutPage.qml",
                props: {}
            }
        })

    onPageChanged: {
        // Off first: a Loader forgets the properties it made its last page
        // with, so they are given again each time.
        pageLoader.active = false;
        const lazy = root.lazyPages[root.page];
        if (lazy) {
            pageLoader.setSource(lazy.url, lazy.props);
            pageLoader.active = true;
        }
    }

    // A model downloaded or deleted: the chat's list follows.
    Connections {
        target: root.models
        function onModelsChanged() {
            root.chat.refreshModels();
        }
    }

    // The conversations' menu, made the first time it is asked for.
    Loader {
        id: conversationMenu
        property string conversationId
        property string conversationTitle
        readonly property var menu: conversationMenu.item

        active: false

        function show(parentItem, x, y) {
            conversationMenu.active = true;
            conversationMenu.menu.popup(parentItem, x, y);
        }

        sourceComponent: ContextMenu {
            ContextMenuItem {
                text: qsTr("Delete…")
                symbol: Symbols.Delete
                destructive: true
                onTriggered: root.confirmDelete(conversationMenu.conversationId, conversationMenu.conversationTitle)
            }
        }
    }

    Component.onCompleted: chatView.focusComposer()
}
