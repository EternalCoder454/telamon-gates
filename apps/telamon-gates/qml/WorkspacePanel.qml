pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import Telamon.Ui

// The coding workspace beside the chat (Code and Agent mode): the files of the
// conversation's folder, an editor with tabs, and a console for running code.
// Made the first time the panel is opened and dropped when it is closed
// (Main.qml). Everything here is the user's own work or the agent's, shown
// as plain text: a file is never run or taken for markup, and nothing is
// opened outside the folder (the files go through `workbench`, which checks
// every path).
Item {
    id: panel

    required property var chat
    required property var workbench

    // Asked to close (the chat header's button is hidden when the window is
    // too narrow for both).
    signal closeRequested

    // True while some tab has changes that are not saved.
    readonly property bool hasUnsaved: panel.unsavedCount > 0
    property int unsavedCount: 0

    // The tabs: one per file, in the order opened. `modified` is the dot.
    ListModel {
        id: tabs
    }
    property int current: -1
    // Text waiting for the editor of a new tab to exist: path to
    // {text, marks, scroll}.
    property var pending: ({})
    // What each save sent, to tell whether the editor changed since.
    property var sent: ({})

    // The tree: a flat list of what is shown, folders opened in place.
    ListModel {
        id: rows
    }
    // The folders that are open, by path.
    property var openDirs: ({})
    property string selectedPath: ""
    // A file to show in the tree once its folders are open.
    property string revealPath: ""

    // What went wrong last, in one line.
    property string message: ""

    readonly property string folderName: {
        const root = panel.workbench.root;
        if (panel.chat.sandboxed) {
            return qsTr("Sandbox");
        }
        return root.length > 0 ? root.substring(root.lastIndexOf("/") + 1) : "";
    }

    // ------------------------------------------------------------ the folder

    function attach() {
        panel.resetView();
        panel.workbench.attach(panel.chat.conversationId, panel.chat.workspace, panel.chat.agentNetwork, panel.chat.agentHome);
    }

    function resetView() {
        tabs.clear();
        rows.clear();
        panel.current = -1;
        panel.unsavedCount = 0;
        panel.pending = ({});
        panel.sent = ({});
        panel.openDirs = ({});
        panel.selectedPath = "";
        panel.revealPath = "";
        panel.message = "";
    }

    function loadRoot() {
        rows.clear();
        panel.openDirs = ({});
        panel.workbench.watch("");
        panel.workbench.list("");
    }

    Component.onCompleted: {
        consoleView.append(panel.workbench.backlog());
        panel.attach();
    }
    // Closed: the folder is let go, and so is a command still running.
    Component.onDestruction: panel.workbench.detach()

    Connections {
        target: panel.chat
        function onConversationIdChanged() {
            Qt.callLater(panel.attach);
        }
        function onWorkspaceChanged() {
            Qt.callLater(panel.attach);
        }
        function onAgentNetworkChanged() {
            panel.workbench.setAccess(panel.chat.agentNetwork, panel.chat.agentHome);
        }
        function onAgentHomeChanged() {
            panel.workbench.setAccess(panel.chat.agentNetwork, panel.chat.agentHome);
        }
    }

    Connections {
        target: panel.workbench

        function onReadyChanged() {
            if (panel.workbench.ready) {
                panel.loadRoot();
            }
        }
        function onListed(dir, json, more) {
            panel.showListing(dir, JSON.parse(json), more);
        }
        function onListFailed(dir, error) {
            if (dir === "") {
                panel.message = error;
                return;
            }
            const i = panel.rowOf(dir);
            if (i >= 0) {
                panel.collapse(i);
            }
        }
        function onChanged(json) {
            panel.refresh(JSON.parse(json));
        }
        function onOpened(path, text, crlf, readOnly, bidi) {
            panel.opened(path, text, crlf, readOnly, bidi);
        }
        function onOpenFailed(path, error) {
            panel.message = error;
        }
        function onSaved(path) {
            panel.saved(path);
        }
        function onCreated(path) {
            panel.openFile(path);
        }
        function onSaveFailed(path, error) {
            panel.message = error;
            delete panel.sent[path];
        }
        function onLiveEdit(path, text, hasText, marks, scrollTo) {
            panel.liveEdit(path, text, hasText, JSON.parse(marks), scrollTo);
        }
        function onTouched(path) {
            panel.reveal(path);
        }
        function onConsoleText(text) {
            consoleView.append(text);
        }
        function onConsoleCleared() {
            consoleView.clear();
        }
    }

    // ------------------------------------------------------------- the tree

    function rowOf(path) {
        for (let i = 0; i < rows.count; ++i) {
            if (rows.get(i).path === path && !rows.get(i).more) {
                return i;
            }
        }
        return -1;
    }

    // The entries of folder `dir` arrived: they replace what the tree showed
    // under it. Folders that were open are listed again.
    function showListing(dir, entries, more) {
        let at = 0;
        let depth = 0;
        if (dir !== "") {
            const i = panel.rowOf(dir);
            if (i < 0 || !rows.get(i).expanded) {
                return;
            }
            at = i + 1;
            depth = rows.get(i).depth + 1;
        }
        let end = at;
        while (end < rows.count && rows.get(end).depth >= depth) {
            ++end;
        }
        if (end > at) {
            rows.remove(at, end - at);
        }
        const reopen = [];
        for (let k = 0; k < entries.length; ++k) {
            const e = entries[k];
            const path = dir === "" ? e.n : dir + "/" + e.n;
            const open = e.d && panel.openDirs[path] === true;
            rows.insert(at + k, {
                path: path,
                name: e.n,
                depth: depth,
                dir: e.d,
                expanded: open,
                more: 0
            });
            if (open) {
                reopen.push(path);
            }
        }
        if (more > 0) {
            rows.insert(at + entries.length, {
                path: "",
                name: qsTr("… and %1 more").arg(more),
                depth: depth,
                dir: false,
                expanded: false,
                more: more
            });
        }
        for (const path of reopen) {
            panel.workbench.list(path);
        }
        panel.continueReveal();
    }

    function expand(i) {
        const r = rows.get(i);
        rows.setProperty(i, "expanded", true);
        panel.openDirs[r.path] = true;
        panel.workbench.watch(r.path);
        panel.workbench.list(r.path);
    }

    function collapse(i) {
        const r = rows.get(i);
        rows.setProperty(i, "expanded", false);
        delete panel.openDirs[r.path];
        panel.workbench.unwatch(r.path);
        let end = i + 1;
        while (end < rows.count && rows.get(end).depth > r.depth) {
            const child = rows.get(end);
            if (child.expanded) {
                delete panel.openDirs[child.path];
                panel.workbench.unwatch(child.path);
            }
            ++end;
        }
        if (end > i + 1) {
            rows.remove(i + 1, end - i - 1);
        }
    }

    // A click or Return on row `i`: a folder opens or closes, a file opens in
    // the editor.
    function activate(i) {
        const r = rows.get(i);
        if (r.more > 0) {
            return;
        }
        panel.selectedPath = r.path;
        if (r.dir) {
            if (r.expanded) {
                panel.collapse(i);
            } else {
                panel.expand(i);
            }
        } else {
            panel.openFile(r.path);
        }
    }

    // Something changed in these folders (or `*`: anywhere): the open ones
    // are listed again.
    function refresh(dirs) {
        if (!panel.workbench.ready) {
            return;
        }
        const all = dirs.indexOf("*") >= 0;
        if (all || dirs.indexOf("") >= 0) {
            panel.workbench.list("");
        }
        for (const dir of Object.keys(panel.openDirs)) {
            if (all || dirs.indexOf(dir) >= 0) {
                panel.workbench.list(dir);
            }
        }
    }

    // Shows file `path` in the tree, opening its folders one by one as their
    // lists arrive.
    function reveal(path) {
        panel.selectedPath = path;
        panel.revealPath = path;
        panel.continueReveal();
    }

    function continueReveal() {
        const path = panel.revealPath;
        if (path.length === 0) {
            return;
        }
        const parts = path.split("/");
        let dir = "";
        for (let k = 0; k < parts.length - 1; ++k) {
            dir = dir === "" ? parts[k] : dir + "/" + parts[k];
            const i = panel.rowOf(dir);
            if (i < 0) {
                return;
            }
            if (!rows.get(i).expanded) {
                panel.expand(i);
                return;
            }
        }
        const at = panel.rowOf(path);
        if (at >= 0) {
            tree.positionViewAtIndex(at, ListView.Contain);
            panel.revealPath = "";
        }
    }

    // ------------------------------------------------------------- the tabs

    // The editor of tab `i`.
    function paneAt(i) {
        return editors.itemAt(i) as EditorPane;
    }

    function tabOf(path) {
        for (let i = 0; i < tabs.count; ++i) {
            if (tabs.get(i).path === path) {
                return i;
            }
        }
        return -1;
    }

    function openFile(path) {
        const i = panel.tabOf(path);
        if (i >= 0) {
            panel.current = i;
            return;
        }
        panel.workbench.open(path);
    }

    // A file read from disk: a new tab, or, for a file that is open, its new
    // text.
    function opened(path, text, crlf, readOnly, bidi) {
        const i = panel.tabOf(path);
        if (i < 0) {
            panel.addTab(path, text, crlf, readOnly, bidi, [], 0);
            return;
        }
        const pane = panel.paneAt(i);
        pane.code.setTextPreserving(text);
        pane.code.modified = false;
        panel.current = i;
    }

    function addTab(path, text, crlf, readOnly, bidi, marks, scroll) {
        panel.pending[path] = {
            text: text,
            marks: marks,
            scroll: scroll
        };
        tabs.append({
            path: path,
            title: path.substring(path.lastIndexOf("/") + 1),
            toolTip: path,
            modified: false,
            crlf: crlf,
            locked: readOnly,
            bidi: bidi
        });
        panel.current = tabs.count - 1;
        panel.selectedPath = path;
    }

    // The agent changed file `path`: its tab shows the new text where it
    // is, the changed lines marked for a moment, and scrolls to the first.
    function liveEdit(path, text, hasText, marks, scrollTo) {
        const i = panel.tabOf(path);
        if (i < 0) {
            if (hasText) {
                panel.addTab(path, text, false, false, false, marks, scrollTo);
            } else {
                panel.workbench.open(path);
            }
        } else if (hasText) {
            const pane = panel.paneAt(i);
            pane.code.setTextPreserving(text);
            // What the editor holds is what is on disk.
            pane.code.modified = false;
            pane.applyMarks(marks, scrollTo);
            panel.current = i;
        } else {
            panel.workbench.open(path);
        }
        panel.reveal(path);
    }

    // Tab `i` has (or has no longer) unsaved changes.
    function noteModified(i, modified) {
        if (i < 0 || i >= tabs.count) {
            return;
        }
        tabs.setProperty(i, "modified", modified);
        panel.workbench.markUnsaved(tabs.get(i).path, modified);
        let n = 0;
        for (let k = 0; k < tabs.count; ++k) {
            if (tabs.get(k).modified) {
                ++n;
            }
        }
        panel.unsavedCount = n;
    }

    function save(i) {
        if (i < 0 || i >= tabs.count) {
            return;
        }
        const tab = tabs.get(i);
        const pane = panel.paneAt(i);
        if (!pane || !tab.modified || pane.code.readOnly) {
            return;
        }
        const text = pane.code.text;
        panel.sent[tab.path] = text;
        panel.workbench.save(tab.path, text, tab.crlf);
    }

    function saveAll() {
        for (let i = 0; i < tabs.count; ++i) {
            panel.save(i);
        }
    }

    function saved(path) {
        const i = panel.tabOf(path);
        if (i < 0) {
            return;
        }
        const pane = panel.paneAt(i);
        // Typed since the save was sent: still not saved.
        if (pane.code.text === panel.sent[path]) {
            pane.code.modified = false;
        }
        delete panel.sent[path];
        panel.workbench.markUnsaved(path, pane.code.modified);
    }

    function closeTab(i) {
        if (i < 0 || i >= tabs.count) {
            return;
        }
        panel.workbench.markUnsaved(tabs.get(i).path, false);
        tabs.remove(i);
        if (panel.current > i || panel.current >= tabs.count) {
            panel.current = panel.current - 1;
        }
        let n = 0;
        for (let k = 0; k < tabs.count; ++k) {
            if (tabs.get(k).modified) {
                ++n;
            }
        }
        panel.unsavedCount = n;
    }

    function askClose(i) {
        if (tabs.get(i).modified) {
            discard.index = i;
            discard.open();
        } else {
            panel.closeTab(i);
        }
    }

    ConfirmDialog {
        id: discard
        property int index: -1
        title: qsTr("Close Without Saving?")
        text: qsTr("The changes in this file are not saved.")
        acceptText: qsTr("Close")
        destructive: true
        onAccepted: panel.closeTab(discard.index)
    }

    // A new, empty file.
    ConfirmDialog {
        id: newFile
        title: qsTr("New File")
        acceptText: qsTr("Create")
        onAboutToShow: {
            nameField.text = "";
            nameField.forceActiveFocus();
        }
        onAccepted: panel.workbench.create(nameField.text.trim())

        TelamonTextField {
            id: nameField
            placeholderText: qsTr("Name, such as src/main.rs")
            Accessible.name: qsTr("File name")
        }
    }

    Shortcut {
        sequences: [StandardKey.Save]
        enabled: panel.visible && panel.current >= 0
        onActivated: panel.save(panel.current)
    }

    // The editor of a tab. `code` is the editor; the text of a new tab is
    // put in when it is made.
    component EditorPane: ColumnLayout {
        id: pane

        required property int index
        required property string path
        required property bool locked
        required property bool bidi
        readonly property alias code: editor
        // Marks to draw once a big text has arrived.
        property var deferred: null

        spacing: 0

        function applyMarks(marks, scroll) {
            if (editor.loading) {
                pane.deferred = {
                    marks: marks,
                    scroll: scroll
                };
                return;
            }
            const added = [];
            const changed = [];
            for (const m of marks) {
                (m[2] ? added : changed).push([m[0], m[1]]);
            }
            if (added.length > 0) {
                editor.markLines(added, TelamonCodeEditor.Added, 6000);
            }
            if (changed.length > 0) {
                editor.markLines(changed, TelamonCodeEditor.Changed, 6000);
            }
            if (scroll > 0) {
                editor.scrollToLine(scroll);
            }
        }

        InfoBanner {
            Layout.fillWidth: true
            type: "warning"
            shown: pane.bidi
            text: qsTr("This file has bidirectional control characters, which can make code read in another order than it runs. They are kept as they are.")
        }
        InfoBanner {
            Layout.fillWidth: true
            type: "info"
            shown: pane.locked
            text: qsTr("This file is read-only here: its lines end in different ways, and the editor would change them.")
        }
        TelamonCodeEditor {
            id: editor
            Layout.fillWidth: true
            Layout.fillHeight: true
            fileName: pane.path
            readOnly: pane.locked
            Accessible.name: pane.path
            onModifiedChanged: panel.noteModified(pane.index, editor.modified)
            onLoaded: {
                if (pane.deferred !== null) {
                    const d = pane.deferred;
                    pane.deferred = null;
                    pane.applyMarks(d.marks, d.scroll);
                }
            }
            Component.onCompleted: {
                const job = panel.pending[pane.path];
                delete panel.pending[pane.path];
                if (job) {
                    editor.text = job.text;
                    pane.applyMarks(job.marks, job.scroll);
                }
            }
        }
    }

    // ------------------------------------------------------------ running

    function run() {
        const command = commandField.text.trim();
        if (command.length === 0) {
            return;
        }
        // The code that runs is the code shown.
        panel.saveAll();
        panel.chat.saveRunCommand(commandField.text);
        panel.workbench.run(command);
    }

    // ------------------------------------------------------------ the view

    ColumnLayout {
        anchors.fill: parent
        anchors.topMargin: TelamonStyle.spacingXLarge + TelamonStyle.spacingLarge
        anchors.leftMargin: TelamonStyle.spacingLarge
        anchors.rightMargin: TelamonStyle.spacingLarge
        anchors.bottomMargin: Kirigami.Units.largeSpacing * 2
        spacing: TelamonStyle.spacing

        RowLayout {
            Layout.fillWidth: true
            spacing: TelamonStyle.spacing

            Symbol {
                icon: panel.chat.sandboxed ? Symbols.Shield : Symbols.FolderOpen
                color: TelamonStyle.accent
            }
            TelamonLabel {
                Layout.fillWidth: true
                textStyle: TelamonLabel.Title
                textFormat: Text.PlainText
                elide: Text.ElideMiddle
                text: qsTr("Workspace")
                Accessible.description: panel.workbench.root
            }
            TelamonLabel {
                textFormat: Text.PlainText
                elide: Text.ElideMiddle
                Layout.maximumWidth: panel.width * 0.4
                opacity: 0.7
                text: panel.folderName
            }
            ToolbarButton {
                symbol: Symbols.Save
                text: qsTr("Save")
                shortcutText: "Ctrl+S"
                focusable: true
                enabled: panel.current >= 0 && panel.current < tabs.count && tabs.get(panel.current).modified
                onClicked: panel.save(panel.current)
            }
            ToolbarButton {
                symbol: Symbols.Refresh
                text: qsTr("Refresh")
                focusable: true
                enabled: panel.workbench.ready
                onClicked: panel.refresh(["*"])
            }
            ToolbarButton {
                symbol: Symbols.Close
                text: qsTr("Close Workspace")
                focusable: true
                onClicked: panel.closeRequested()
            }
        }

        InfoBanner {
            Layout.fillWidth: true
            type: "error"
            closable: true
            shown: panel.message.length > 0
            text: panel.message
            onClosed: panel.message = ""
        }

        TelamonEmptyState {
            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: panel.workbench.problem.length > 0
            symbol: Symbols.FolderOff
            title: qsTr("No Workspace Yet")
            text: panel.workbench.problem
        }

        TelamonSplitView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: panel.workbench.problem.length === 0
            orientation: Qt.Vertical
            stateKey: "workspace-rows"

            TelamonSplitView {
                QQC2.SplitView.fillHeight: true
                QQC2.SplitView.minimumHeight: Kirigami.Units.gridUnit * 8
                stateKey: "workspace-columns"

                // The files.
                Item {
                    QQC2.SplitView.preferredWidth: Kirigami.Units.gridUnit * 11
                    QQC2.SplitView.minimumWidth: Kirigami.Units.gridUnit * 7

                    ListView {
                        id: tree
                        anchors.fill: parent
                        anchors.rightMargin: TelamonStyle.spacingSmall
                        model: rows
                        clip: true
                        boundsBehavior: Flickable.StopAtBounds
                        activeFocusOnTab: true
                        keyNavigationEnabled: true
                        QQC2.ScrollBar.vertical: TelamonScrollBar {}
                        Accessible.role: Accessible.Tree
                        Accessible.name: qsTr("Files")

                        Keys.onReturnPressed: panel.activate(tree.currentIndex)
                        Keys.onEnterPressed: panel.activate(tree.currentIndex)
                        Keys.onSpacePressed: panel.activate(tree.currentIndex)
                        Keys.onRightPressed: {
                            const i = tree.currentIndex;
                            if (i >= 0 && rows.get(i).dir && !rows.get(i).expanded) {
                                panel.expand(i);
                            }
                        }
                        Keys.onLeftPressed: {
                            const i = tree.currentIndex;
                            if (i >= 0 && rows.get(i).dir && rows.get(i).expanded) {
                                panel.collapse(i);
                            }
                        }

                        delegate: Item {
                            id: row

                            required property int index
                            required property string path
                            required property string name
                            required property int depth
                            required property bool dir
                            required property bool expanded
                            required property int more
                            readonly property bool selected: row.path.length > 0 && row.path === panel.selectedPath

                            width: ListView.view.width
                            height: TelamonStyle.rowHeight
                            Accessible.role: Accessible.TreeItem
                            Accessible.name: row.name
                            Accessible.selected: row.selected

                            Rectangle {
                                anchors.fill: parent
                                anchors.topMargin: 1
                                anchors.bottomMargin: 1
                                radius: TelamonStyle.radiusSmall
                                color: row.selected ? Qt.alpha(TelamonStyle.accent, 0.18) : hover.hovered && row.more === 0 ? Qt.alpha(Kirigami.Theme.textColor, 0.07) : "transparent"
                                border.width: tree.activeFocus && tree.currentIndex === row.index ? 1 : 0
                                border.color: TelamonStyle.accent
                            }
                            RowLayout {
                                anchors.fill: parent
                                anchors.leftMargin: TelamonStyle.spacingSmall + row.depth * Kirigami.Units.gridUnit
                                anchors.rightMargin: TelamonStyle.spacingSmall
                                spacing: TelamonStyle.spacingSmall

                                Item {
                                    Layout.preferredWidth: Kirigami.Units.iconSizes.small
                                    Layout.preferredHeight: Kirigami.Units.iconSizes.small
                                    Symbol {
                                        anchors.centerIn: parent
                                        visible: row.dir
                                        icon: row.expanded ? Symbols.ExpandMore : Symbols.ChevronRight
                                        size: Kirigami.Units.iconSizes.small
                                        opacity: 0.7
                                    }
                                }
                                Symbol {
                                    visible: row.more === 0
                                    icon: row.dir ? (row.expanded ? Symbols.FolderOpen : Symbols.Folder) : Symbols.Description
                                    size: Kirigami.Units.iconSizes.smallMedium
                                    color: row.dir ? TelamonStyle.accent : Kirigami.Theme.textColor
                                    opacity: row.dir ? 1 : 0.7
                                }
                                TelamonLabel {
                                    Layout.fillWidth: true
                                    // File names are plain text.
                                    textFormat: Text.PlainText
                                    elide: Text.ElideMiddle
                                    text: row.name
                                    opacity: row.more > 0 ? 0.6 : 1
                                }
                            }
                            HoverHandler {
                                id: hover
                            }
                            TapHandler {
                                onTapped: {
                                    tree.currentIndex = row.index;
                                    tree.forceActiveFocus();
                                    panel.activate(row.index);
                                }
                            }
                        }

                        TelamonEmptyState {
                            anchors.centerIn: parent
                            width: parent.width
                            visible: panel.workbench.ready && rows.count === 0
                            symbol: Symbols.FolderOpen
                            title: qsTr("Empty Folder")
                            text: qsTr("Files made here show up in this list.")
                        }
                    }
                }

                // The editor.
                ColumnLayout {
                    QQC2.SplitView.fillWidth: true
                    QQC2.SplitView.minimumWidth: Kirigami.Units.gridUnit * 12
                    spacing: TelamonStyle.spacingSmall

                    TabBar {
                        id: tabBar
                        Layout.fillWidth: true
                        visible: tabs.count > 0
                        density: TelamonStyle.Compact
                        model: tabs
                        currentIndex: panel.current
                        onActivated: index => panel.current = index
                        onCloseRequested: index => panel.askClose(index)
                        onNewRequested: newFile.open()
                    }

                    StackLayout {
                        id: stack
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        currentIndex: panel.current

                        Repeater {
                            id: editors
                            model: tabs
                            delegate: EditorPane {}
                        }
                    }

                    TelamonEmptyState {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        visible: tabs.count === 0
                        symbol: Symbols.Code
                        title: qsTr("No File Open")
                        text: qsTr("Pick a file from the list. When the agent changes a file, it opens here.")
                        actionText: qsTr("New File")
                        actionSymbol: Symbols.NoteAdd
                        onTriggered: newFile.open()
                    }
                }
            }

            // Run, and what it prints.
            ColumnLayout {
                QQC2.SplitView.preferredHeight: Kirigami.Units.gridUnit * 13
                QQC2.SplitView.minimumHeight: Kirigami.Units.gridUnit * 7
                spacing: TelamonStyle.spacingSmall

                RowLayout {
                    Layout.fillWidth: true
                    Layout.topMargin: TelamonStyle.spacingSmall
                    spacing: TelamonStyle.spacing

                    TelamonTextField {
                        id: commandField
                        Layout.fillWidth: true
                        placeholderText: qsTr("Command, such as cargo run or python3 main.py")
                        text: panel.chat.runCommand
                        enabled: !panel.workbench.running
                        Accessible.name: qsTr("Command to run")
                        onAccepted: panel.run()
                        onEditingFinished: panel.chat.saveRunCommand(commandField.text)
                    }
                    PrimaryButton {
                        visible: !panel.workbench.running
                        text: qsTr("Run")
                        symbol: Symbols.PlayArrow
                        enabled: panel.workbench.ready && panel.workbench.commands && commandField.text.trim().length > 0
                        onClicked: panel.run()
                    }
                    SecondaryButton {
                        visible: panel.workbench.running
                        text: qsTr("Stop")
                        symbol: Symbols.Stop
                        onClicked: panel.workbench.stop()
                    }
                    ToolbarButton {
                        symbol: Symbols.DeleteSweep
                        text: qsTr("Clear Console")
                        focusable: true
                        onClicked: panel.workbench.clearConsole()
                    }
                }
                TelamonLabel {
                    Layout.fillWidth: true
                    textFormat: Text.PlainText
                    textStyle: TelamonLabel.Caption
                    wrapMode: Text.Wrap
                    opacity: 0.75
                    text: !panel.workbench.commands ? qsTr("Commands are off: they run in a bubblewrap sandbox, and bubblewrap isn't installed.") : qsTr("Runs in the sandbox, in this folder. Network: %1. Your home folder: %2.").arg(panel.chat.agentNetwork ? qsTr("allowed") : qsTr("off")).arg(panel.chat.agentHome ? qsTr("readable") : qsTr("hidden"))
                }
                // Plain text: what a command prints is never taken for markup.
                TelamonConsoleView {
                    id: consoleView
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    maximumLines: 5000
                    Accessible.name: qsTr("Console output")
                }
            }
        }
    }
}
