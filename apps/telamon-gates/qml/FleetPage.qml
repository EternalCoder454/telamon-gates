pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Dialogs
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import Telamon.Ui

// The Fleet: a goal and a folder, and the agents that work on it, each a
// card that shows what it is doing. The fleet runs one agent at a time; any
// of them can be stopped, and each change an agent asks for waits here for
// your answer. Everything an agent or the coordinator wrote is untrusted
// text: it is only ever shown as plain text.
TelamonPage {
    id: page

    // Set from Main.qml; see src/fleet.rs and src/chat.rs.
    required property var fleet
    required property var chat

    readonly property int total: page.fleet.titles.length
    readonly property int finished: page.count("done") + page.count("failed") + page.count("stopped")
    readonly property bool noTools: page.chat.serverUrl.length === 0 && page.chat.models.length > 0 && page.chat.toolModels.indexOf(page.chat.model) < 0

    title: qsTr("Fleet")
    subtitle: qsTr("Several agents work on one goal, one after another. You watch them, and you allow or deny every change.")
    maxContentWidth: Kirigami.Units.gridUnit * 56

    // How many agents have this status.
    function count(status) {
        let n = 0;
        for (const s of page.fleet.statuses) {
            if (s === status) {
                ++n;
            }
        }
        return n;
    }

    function steps() {
        let n = 0;
        for (const s of page.fleet.steps) {
            n += s;
        }
        return n;
    }

    // The fastest agent's tokens per second now: only one works at a time.
    function speed() {
        let best = 0;
        for (const s of page.fleet.speeds) {
            best = Math.max(best, s);
        }
        return best;
    }

    function badgeText(status) {
        switch (status) {
        case "working":
            return qsTr("Working");
        case "waiting":
            return qsTr("Waiting for You");
        case "done":
            return qsTr("Done");
        case "failed":
            return qsTr("Failed");
        case "stopped":
            return qsTr("Stopped");
        }
        return qsTr("Idle");
    }

    function badgeType(status) {
        switch (status) {
        case "working":
            return "accent";
        case "waiting":
            return "warning";
        case "done":
            return "success";
        case "failed":
            return "error";
        }
        return "neutral";
    }

    headerTrailing: [
        PrimaryButton {
            visible: !page.fleet.running
            text: qsTr("Start Fleet")
            symbol: Symbols.PlayArrow
            enabled: goalField.text.trim().length > 0 && page.fleet.workspace.length > 0
            onClicked: page.fleet.start(goalField.text)
        },
        SecondaryButton {
            visible: page.fleet.running
            text: qsTr("Stop All")
            symbol: Symbols.Stop
            onClicked: page.fleet.stop()
        }
    ]

    Component.onCompleted: goalField.text = page.fleet.goal

    // A dot that glows outward while its agent works.
    component PulseDot: Item {
        id: dot
        required property string status
        readonly property color tone: dot.status === "working" ? TelamonStyle.accent : dot.status === "waiting" ? TelamonStyle.warning : dot.status === "done" ? TelamonStyle.success : dot.status === "failed" ? TelamonStyle.error : TelamonStyle.textMuted
        readonly property bool pulsing: dot.status === "working" && !TelamonStyle.reducedMotion

        implicitWidth: Kirigami.Units.gridUnit
        implicitHeight: Kirigami.Units.gridUnit
        Accessible.ignored: true

        Rectangle {
            id: halo
            anchors.centerIn: parent
            width: Kirigami.Units.gridUnit * 0.5
            height: width
            radius: width / 2
            color: dot.tone
            opacity: 0
        }
        Rectangle {
            anchors.centerIn: parent
            width: Kirigami.Units.gridUnit * 0.5
            height: width
            radius: width / 2
            color: dot.tone
            opacity: dot.status === "idle" || dot.status === "stopped" ? 0.5 : 1
        }
        ParallelAnimation {
            running: dot.pulsing && dot.visible
            loops: Animation.Infinite
            onRunningChanged: {
                if (!running) {
                    halo.opacity = 0;
                    halo.scale = 1;
                }
            }
            NumberAnimation {
                target: halo
                property: "scale"
                from: 1
                to: 2.6
                duration: TelamonStyle.durationLong * 6
                easing.type: Easing.OutCubic
            }
            NumberAnimation {
                target: halo
                property: "opacity"
                from: 0.55
                to: 0
                duration: TelamonStyle.durationLong * 6
                easing.type: Easing.OutCubic
            }
        }
    }

    InfoBanner {
        Layout.fillWidth: true
        type: "error"
        text: page.fleet.error
        shown: page.fleet.error.length > 0
        closable: true
        onClosed: page.fleet.dismissError()
    }

    InfoBanner {
        Layout.fillWidth: true
        text: qsTr("The demo answers with samples, so it can't plan work for a fleet. Install telamon-llama and get a model first.")
        shown: page.fleet.demo
    }

    InfoBanner {
        Layout.fillWidth: true
        type: "warning"
        text: qsTr("%1 wasn't made to use tools, so the agents can only talk. Pick a model that can, such as Qwen3.").arg(page.chat.model)
        shown: page.noTools && !page.fleet.demo
    }

    // The goal and the folder.
    TelamonCard {
        title: qsTr("Goal")
        subtitle: page.fleet.running ? "" : qsTr("A coordinator splits it into a few tasks, and an agent takes each one.")

        // While a run is under way the goal is only read: a line or two, so
        // the agents have the room.
        TelamonLabel {
            visible: page.fleet.running
            Layout.fillWidth: true
            wrapMode: Text.Wrap
            maximumLineCount: 3
            elide: Text.ElideRight
            text: page.fleet.goal
        }
        TelamonTextArea {
            id: goalField
            visible: !page.fleet.running
            Layout.fillWidth: true
            implicitHeight: Kirigami.Units.gridUnit * 5
            placeholderText: qsTr("For example: add input checks to the signup form, write tests for them, and update the README.")
            Accessible.name: qsTr("Goal")
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: TelamonStyle.spacing

            Symbol {
                icon: Symbols.FolderOpen
                color: TelamonStyle.accent
            }
            TelamonLabel {
                Layout.fillWidth: true
                elide: Text.ElideMiddle
                opacity: page.fleet.workspace.length > 0 ? 1 : 0.7
                text: page.fleet.workspace.length > 0 ? page.fleet.workspace : qsTr("Choose the folder the agents work in. They can read anything there, and ask before they change a file or run a command.")
                wrapMode: page.fleet.workspace.length > 0 ? Text.NoWrap : Text.Wrap
            }
            SecondaryButton {
                text: page.fleet.workspace.length > 0 ? qsTr("Change…") : qsTr("Choose Folder…")
                enabled: !page.fleet.running
                symbol: Symbols.FolderOpen
                onClicked: picker.open()
            }
        }
    }

    FolderDialog {
        id: picker
        title: qsTr("Folder for the Fleet")
        onAccepted: page.fleet.chooseWorkspace(decodeURIComponent(selectedFolder.toString().replace(/^file:\/\//, "")))
    }

    // The question: which agent wants to do what, and the answers.
    Rectangle {
        visible: page.fleet.approving
        Layout.fillWidth: true
        implicitHeight: ask.implicitHeight + TelamonStyle.spacingLarge * 2
        radius: TelamonStyle.radiusLarge
        color: TelamonStyle.control
        border.width: 1
        border.color: TelamonStyle.warning

        ColumnLayout {
            id: ask
            anchors.fill: parent
            anchors.margins: TelamonStyle.spacingLarge
            spacing: TelamonStyle.spacing

            RowLayout {
                spacing: TelamonStyle.spacing

                Symbol {
                    icon: page.fleet.approvalKind === "run" ? Symbols.Terminal : Symbols.EditNote
                    color: TelamonStyle.warning
                }
                TelamonLabel {
                    Layout.fillWidth: true
                    elide: Text.ElideMiddle
                    font.weight: Font.DemiBold
                    text: qsTr("%1 asks: %2").arg(page.fleet.approvalAgent).arg(page.fleet.approvalTitle)
                }
            }
            // What the model wrote: plain text, never run or shown as HTML.
            TelamonCodeView {
                Layout.fillWidth: true
                text: page.fleet.approvalDetail
                maximumHeight: Kirigami.Units.gridUnit * 10
                // Wrapped: a long line can't hide its end off to the side.
                wrap: true
                Accessible.name: page.fleet.approvalKind === "run" ? qsTr("Command") : qsTr("Change")
            }
            RowLayout {
                Layout.alignment: Qt.AlignRight
                spacing: TelamonStyle.spacing

                SecondaryButton {
                    text: qsTr("Deny")
                    onClicked: page.fleet.answerApproval(0)
                }
                SecondaryButton {
                    visible: page.fleet.approvalKind === "write"
                    text: qsTr("Allow All Edits by This Agent")
                    onClicked: page.fleet.answerApproval(2)
                }
                PrimaryButton {
                    text: page.fleet.approvalKind === "run" ? qsTr("Run") : qsTr("Allow")
                    onClicked: page.fleet.answerApproval(1)
                }
            }
        }
    }

    // The coordinator is splitting the goal.
    TelamonCard {
        visible: page.fleet.planning

        RowLayout {
            Layout.fillWidth: true
            spacing: TelamonStyle.spacingLarge

            TelamonSpinner {
                running: page.fleet.planning
                Layout.alignment: Qt.AlignVCenter
                Layout.preferredWidth: Kirigami.Units.iconSizes.medium
                Layout.preferredHeight: Kirigami.Units.iconSizes.medium
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 0

                TelamonLabel {
                    Layout.fillWidth: true
                    font.weight: Font.DemiBold
                    text: qsTr("Planning")
                }
                TelamonLabel {
                    Layout.fillWidth: true
                    textStyle: TelamonLabel.Caption
                    wrapMode: Text.Wrap
                    text: qsTr("The coordinator is splitting the goal into tasks.")
                }
            }
        }
    }

    // How it is going, in figures.
    TelamonCard {
        visible: page.total > 0
        padding: TelamonStyle.spacingXLarge

        GridLayout {
            Layout.fillWidth: true
            columns: width < Kirigami.Units.gridUnit * 30 ? 2 : 4
            columnSpacing: TelamonStyle.spacingXXLarge
            rowSpacing: TelamonStyle.spacingLarge

            TelamonStat {
                Layout.fillWidth: true
                symbol: Symbols.Hub
                label: qsTr("Agents")
                value: String(page.total)
            }
            TelamonStat {
                Layout.fillWidth: true
                symbol: Symbols.TaskAlt
                label: qsTr("Done")
                value: qsTr("%1 of %2").arg(page.count("done")).arg(page.total)
            }
            TelamonStat {
                Layout.fillWidth: true
                symbol: Symbols.Build
                label: qsTr("Steps")
                value: String(page.steps())
            }
            TelamonStat {
                Layout.fillWidth: true
                symbol: Symbols.Speed
                label: qsTr("Speed")
                value: page.speed() > 0 ? Number(page.speed()).toLocaleString(Qt.locale(), "f", 1) : "–"
                unit: page.speed() > 0 ? qsTr("tokens/s") : ""
            }
        }
        TelamonProgressBar {
            Layout.fillWidth: true
            value: page.total > 0 ? page.finished / page.total : 0
            text: qsTr("%1 of %2 finished").arg(page.finished).arg(page.total)
        }
    }

    // The agents.
    GridLayout {
        id: grid
        visible: page.total > 0
        Layout.fillWidth: true
        columnSpacing: TelamonStyle.spacingLarge
        rowSpacing: TelamonStyle.spacingLarge
        uniformCellWidths: true
        uniformCellHeights: true
        columns: Math.max(1, Math.min(3, Math.floor((grid.width + grid.columnSpacing) / (Kirigami.Units.gridUnit * 17 + grid.columnSpacing))))

        // A number, not the list of titles: the cards stay as they are
        // while their figures change.
        Repeater {
            model: page.total

            Item {
                id: cell
                required property int index
                readonly property string status: page.fleet.statuses[cell.index] ?? "idle"
                readonly property string line: page.fleet.lines[cell.index] ?? ""
                readonly property real steps: page.fleet.steps[cell.index] ?? 0
                readonly property real speed: page.fleet.speeds[cell.index] ?? 0
                readonly property real confidence: page.fleet.confidences[cell.index] ?? -1
                readonly property bool open: cell.status === "idle" || cell.status === "working" || cell.status === "waiting"
                readonly property color tone: cell.status === "working" ? TelamonStyle.accent : cell.status === "waiting" ? TelamonStyle.warning : cell.status === "failed" ? TelamonStyle.error : "transparent"

                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.preferredWidth: Kirigami.Units.gridUnit * 17
                implicitWidth: card.implicitWidth
                implicitHeight: card.implicitHeight

                TelamonCard {
                    id: card
                    anchors.fill: parent
                    title: page.fleet.titles[cell.index] ?? ""
                    subtitle: qsTr("Agent %1 of %2").arg(cell.index + 1).arg(page.total)
                    headerTrailing: [
                        PulseDot {
                            status: cell.status
                        },
                        TelamonBadge {
                            type: page.badgeType(cell.status)
                            text: page.badgeText(cell.status)
                        }
                    ]

                    // What it did or said last: two lines at most, and room
                    // for two so the cards stay the same height.
                    Item {
                        Layout.fillWidth: true
                        Layout.preferredHeight: Math.ceil(lineMetrics.lineSpacing) * 2 + TelamonStyle.spacingSmall

                        TelamonLabel {
                            id: lastLine
                            anchors.fill: parent
                            verticalAlignment: Text.AlignTop
                            wrapMode: Text.Wrap
                            maximumLineCount: 2
                            elide: Text.ElideRight
                            color: cell.status === "failed" ? TelamonStyle.error : TelamonStyle.textMuted
                            text: cell.line.length > 0 ? cell.line : cell.status === "idle" ? qsTr("Waits for its turn.") : cell.status === "working" ? qsTr("Starting…") : ""
                        }
                        FontMetrics {
                            id: lineMetrics
                            font: lastLine.font
                        }
                    }
                    TelamonProgressBar {
                        Layout.fillWidth: true
                        indeterminate: cell.status === "working"
                        value: cell.status === "idle" ? 0 : 1
                        status: cell.status === "failed" ? "error" : cell.status === "waiting" || cell.status === "stopped" ? "paused" : "normal"
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        Layout.minimumHeight: stop.implicitHeight
                        spacing: TelamonStyle.spacingLarge

                        Symbol {
                            icon: Symbols.Build
                            size: Kirigami.Units.iconSizes.small
                            color: TelamonStyle.textMuted
                        }
                        TelamonLabel {
                            textStyle: TelamonLabel.Caption
                            text: cell.steps === 1 ? qsTr("1 step") : qsTr("%1 steps").arg(cell.steps)
                        }
                        Symbol {
                            visible: cell.speed > 0
                            icon: Symbols.Speed
                            size: Kirigami.Units.iconSizes.small
                            color: TelamonStyle.textMuted
                        }
                        TelamonLabel {
                            visible: cell.speed > 0
                            textStyle: TelamonLabel.Caption
                            text: qsTr("%1 tokens/s").arg(Number(cell.speed).toLocaleString(Qt.locale(), "f", 1))
                        }
                        Item {
                            Layout.fillWidth: true
                        }
                        SecondaryButton {
                            id: stop
                            visible: page.fleet.running && cell.open
                            text: qsTr("Stop")
                            symbol: Symbols.Stop
                            onClicked: page.fleet.stopAgent(cell.index)
                        }
                    }
                    // SystemOne's view of whether it finished its task.
                    RowLayout {
                        visible: cell.confidence >= 0
                        Layout.fillWidth: true
                        spacing: TelamonStyle.spacing

                        Symbol {
                            icon: Symbols.Bolt
                            size: Kirigami.Units.iconSizes.small
                            color: TelamonStyle.accent
                        }
                        TelamonLabel {
                            Layout.fillWidth: true
                            textStyle: TelamonLabel.Caption
                            elide: Text.ElideRight
                            text: qsTr("SystemOne is %1% sure it finished.").arg(Math.round(cell.confidence * 100))
                        }
                    }
                }

                // The agent's state on the card's edge.
                Rectangle {
                    id: edge
                    anchors.fill: parent
                    radius: TelamonStyle.radius
                    color: "transparent"
                    border.width: 1
                    border.color: cell.tone
                    opacity: 0.85
                    enabled: false

                    SequentialAnimation on opacity {
                        running: cell.status === "working" && !TelamonStyle.reducedMotion && cell.visible
                        loops: Animation.Infinite
                        onRunningChanged: {
                            if (!running) {
                                edge.opacity = 0.85;
                            }
                        }
                        NumberAnimation {
                            to: 0.35
                            duration: TelamonStyle.durationLong * 4
                            easing.type: Easing.InOutSine
                        }
                        NumberAnimation {
                            to: 0.85
                            duration: TelamonStyle.durationLong * 4
                            easing.type: Easing.InOutSine
                        }
                    }
                }
            }
        }
    }

    TelamonEmptyState {
        visible: page.total === 0 && !page.fleet.planning
        Layout.fillWidth: true
        Layout.preferredHeight: Kirigami.Units.gridUnit * 12
        symbol: Symbols.Hub
        title: qsTr("No Agents Yet")
        text: qsTr("Describe a goal and choose a folder. The coordinator splits the goal into tasks, and each agent works on one while you watch.")
    }
}
