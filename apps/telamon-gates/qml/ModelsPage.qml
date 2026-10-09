pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import Telamon.Ui

// The models on this computer, and Hugging Face to get more from.
TelamonPage {
    id: page

    required property var models
    required property var vram
    required property var chat
    // root.confirm, from the window: a ConfirmDialog.
    required property var confirm

    title: qsTr("Models")

    // A size in bytes, in the sidebar's VRAM units.
    function size(bytes) {
        if (bytes >= 1024 * 1024 * 1024) {
            return qsTr("%1 GiB").arg(Number(bytes / (1024 * 1024 * 1024)).toLocaleString(Qt.locale(), "f", 1));
        }
        return qsTr("%1 MiB").arg(Number(bytes / (1024 * 1024)).toLocaleString(Qt.locale(), "f", 0));
    }

    // How a model of `bytes` sits in the graphics card's memory, beside room
    // for the conversation: "fits", "tight" (some layers may run on the
    // processor) or "big" (most of it will). "" when the card is unknown.
    function fit(bytes) {
        if (!page.vram.available || page.vram.total <= 0) {
            return "";
        }
        if (bytes * 1.2 <= page.vram.total) {
            return "fits";
        }
        return bytes <= page.vram.total ? "tight" : "big";
    }

    // A context length in tokens, as models name it: 32768 is "32K", 131072
    // and 128000 "128K", 1048576 "1M".
    function tokens(count) {
        if (count >= 1024 * 1024) {
            return qsTr("%1M").arg(Number(count / (1024 * 1024)).toLocaleString(Qt.locale(), "f", count % (1024 * 1024) === 0 ? 0 : 1));
        }
        return qsTr("%1K").arg(Math.round(count % 1024 === 0 ? count / 1024 : count / 1000));
    }

    function folderUrl(path) {
        return "file://" + path.split("/").map(encodeURIComponent).join("/");
    }

    function confirmDelete(name) {
        page.confirm({
            title: qsTr("Delete Model?"),
            text: qsTr("“%1” will be deleted from this computer.").arg(name),
            acceptText: qsTr("Delete"),
            destructive: true
        }, ok => {
            if (ok) {
                page.models.remove(name);
            }
        });
    }

    TextMetrics {
        id: downloadWidth
        text: qsTr("Download")
    }
    TextMetrics {
        id: downloadedWidth
        text: qsTr("Downloaded")
    }

    // A SectionRow's trailing items sit in a Row, which tops them: these
    // centre themselves beside a taller button.
    component FitBadge: TelamonBadge {
        required property string fit
        y: parent ? Math.round((parent.height - height) / 2) : 0
        visible: fit.length > 0
        type: fit === "fits" ? "success" : fit === "tight" ? "warning" : "error"
        text: fit === "fits" ? qsTr("Fits") : fit === "tight" ? qsTr("Tight") : qsTr("Too Big")
    }

    InfoBanner {
        Layout.fillWidth: true
        type: "error"
        text: page.models.error
        shown: page.models.error.length > 0
        closable: true
        onClosed: page.models.dismissError()
    }

    InfoBanner {
        Layout.fillWidth: true
        text: qsTr("Telamon Gates uses the model server at %1, so models here aren't used.").arg(page.chat.serverUrl)
        shown: page.chat.serverUrl.length > 0
    }

    Section {
        title: qsTr("On This Computer")
        footer: page.vram.available ? qsTr("Fits: room to spare in the graphics card's %1 GiB. Tight: part may run on the processor, slower. Too Big: most of it will.").arg(Number(page.vram.total / (1024 * 1024 * 1024)).toLocaleString(Qt.locale(), "f", 0)) : ""

        Repeater {
            model: page.models.names

            SectionRow {
                id: row
                required property int index
                required property string modelData
                readonly property real bytes: page.models.sizes[index] ?? 0
                // A decision model's kind ("laya"): it answers SystemOne, not chats.
                readonly property string kind: page.models.kinds[index] ?? ""
                // What a chat model can do: its context in tokens (0 when it
                // doesn't say), tools in its template, images through a projector.
                readonly property real context: page.models.contexts[index] ?? 0
                readonly property bool tools: (page.models.toolCapable[index] ?? 0) > 0
                readonly property bool images: (page.models.vision[index] ?? 0) > 0

                title: modelData
                subtitle: [row.kind === "draft" ? qsTr("Draft that speeds up its model") : row.kind.length > 0 ? qsTr("Decision model") : "", page.models.quants[index], page.models.labels[index], row.kind.length === 0 && row.context > 0 ? qsTr("%1 context").arg(page.tokens(row.context)) : "", page.size(bytes)].filter(s => s && s.length > 0).join(" · ")
                leading: [
                    Symbol {
                        icon: Symbols.Psychology
                        color: TelamonStyle.accent
                    }
                ]

                TelamonBadge {
                    y: parent ? Math.round((parent.height - height) / 2) : 0
                    visible: row.kind.length === 0 && row.images
                    symbol: Symbols.Image
                    text: qsTr("Images")
                }
                TelamonBadge {
                    y: parent ? Math.round((parent.height - height) / 2) : 0
                    visible: row.kind.length === 0 && row.tools
                    symbol: Symbols.Build
                    text: qsTr("Tools")
                }
                FitBadge {
                    fit: row.kind.length > 0 ? "" : page.fit(row.bytes)
                }
                TelamonBadge {
                    y: parent ? Math.round((parent.height - height) / 2) : 0
                    visible: row.kind.length > 0
                    type: "accent"
                    symbol: row.kind === "draft" ? Symbols.Speed : Symbols.Bolt
                    text: row.kind === "draft" ? qsTr("Speed-Up") : qsTr("SystemOne")
                }
                ToolbarButton {
                    y: parent ? Math.round((parent.height - height) / 2) : 0
                    symbol: Symbols.Delete
                    text: qsTr("Delete %1").arg(row.modelData)
                    toolTipText: qsTr("Delete")
                    focusable: true
                    onClicked: page.confirmDelete(row.modelData)
                }
            }
        }

        SectionRow {
            visible: page.models.names.length === 0
            title: qsTr("No models yet")
            subtitle: qsTr("Find one below, or put a .gguf file in the models folder.")
            leading: [
                Symbol {
                    icon: Symbols.Inventory2
                    color: TelamonStyle.accent
                }
            ]
        }

        SectionRow {
            title: qsTr("Models Folder")
            subtitle: page.models.folder
            leading: [
                Symbol {
                    icon: Symbols.FolderOpen
                    color: TelamonStyle.accent
                }
            ]

            SecondaryButton {
                text: qsTr("Open Folder")
                symbol: Symbols.FolderOpen
                onClicked: Qt.openUrlExternally(page.folderUrl(page.models.folder))
            }
        }
    }

    // Models tested on Telamon's checks (docs/BACKEND.md → Recommended
    // models): one click gets one, and Use puts it to work. `code` is for
    // Code and Agent mode (Model for Code); the others become the Model.
    // `experts`: a mixture of experts, which still runs, slower, when part
    // of it is in system memory, so it is never Too Big.
    readonly property var recommended: [
        {
            use: qsTr("For Coding"),
            about: qsTr("Qwen3 Coder 30B: the best code in the tests, and quick for its size"),
            repo: "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF",
            file: "Qwen3-Coder-30B-A3B-Instruct-UD-Q4_K_XL.gguf",
            bytes: 17665334432,
            code: true,
            experts: true
        },
        {
            use: qsTr("For Chat and Stories"),
            about: qsTr("gpt-oss 20B: fast, knows a lot, and codes well too"),
            repo: "ggml-org/gpt-oss-20b-GGUF",
            file: "gpt-oss-20b-MXFP4.gguf",
            bytes: 12109566624,
            code: false,
            experts: true
        },
        {
            use: qsTr("Small and Fast"),
            about: qsTr("Qwen3 4B: for graphics cards with 8 GiB or less"),
            repo: "unsloth/Qwen3-4B-Instruct-2507-GGUF",
            file: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
            bytes: 2497281120,
            code: false,
            experts: false
        }
    ]

    TextMetrics {
        id: useWidth
        text: qsTr("In Use")
    }

    Section {
        visible: page.chat.serverUrl.length === 0
        title: qsTr("Recommended")
        footer: qsTr("Tested in Telamon Gates for coding, chat and stories. A coding model can answer Code and Agent mode while another chats.")

        Repeater {
            model: page.recommended

            SectionRow {
                id: pick
                required property var modelData
                readonly property string name: modelData.file.replace(/\.gguf$/, "")
                readonly property bool have: page.models.names.indexOf(name) >= 0
                // The coding pick is in use as the Model for Code, or as the
                // Model when Code uses that.
                readonly property bool inUse: page.chat.model === name && (!modelData.code || page.chat.codeModel.length === 0) || modelData.code && page.chat.codeModel === name
                readonly property bool downloading: page.models.downloading === modelData.file

                title: modelData.use
                subtitle: pick.downloading ? qsTr("Downloading… %1%").arg(Math.floor(page.models.progress * 100)) : modelData.about + " · " + page.size(modelData.bytes)
                busy: pick.downloading
                leading: [
                    Symbol {
                        icon: pick.modelData.code ? Symbols.Code : Symbols.Psychology
                        color: TelamonStyle.accent
                    }
                ]

                FitBadge {
                    readonly property string plain: page.fit(pick.modelData.bytes)
                    fit: pick.modelData.experts && plain === "big" ? "tight" : plain
                }
                SecondaryButton {
                    visible: !pick.have && !pick.downloading
                    implicitWidth: Math.max(downloadWidth.width, useWidth.width) + TelamonStyle.spacingLarge * 4
                    text: qsTr("Download")
                    symbol: Symbols.Download
                    enabled: page.models.downloading.length === 0
                    onClicked: page.models.downloadFrom(pick.modelData.repo, pick.modelData.file)
                }
                SecondaryButton {
                    visible: pick.downloading
                    implicitWidth: Math.max(downloadWidth.width, useWidth.width) + TelamonStyle.spacingLarge * 4
                    text: qsTr("Cancel")
                    symbol: Symbols.Close
                    onClicked: page.models.cancelDownload()
                }
                SecondaryButton {
                    visible: pick.have
                    implicitWidth: Math.max(downloadWidth.width, useWidth.width) + TelamonStyle.spacingLarge * 4
                    text: pick.inUse ? qsTr("In Use") : qsTr("Use")
                    symbol: pick.inUse ? Symbols.Check : Symbols.PlayArrow
                    enabled: !pick.inUse
                    onClicked: pick.modelData.code ? page.chat.pickCodeModel(pick.name) : page.chat.pickModel(pick.name)
                }
            }
        }
    }

    Section {
        title: qsTr("Get Models")
        footer: qsTr("From Hugging Face. Each download is checked against the hash Hugging Face publishes.")

        // The download under way, whatever is open below.
        SectionRow {
            visible: page.models.downloading.length > 0
            leading: [
                Symbol {
                    icon: Symbols.Download
                    color: TelamonStyle.accent
                }
            ]
            content: [
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: TelamonStyle.spacingSmall

                    QQC2.Label {
                        Layout.fillWidth: true
                        text: page.models.downloading
                        textFormat: Text.PlainText
                        elide: Text.ElideMiddle
                    }
                    TelamonProgressBar {
                        Layout.fillWidth: true
                        value: page.models.progress
                        text: qsTr("%1%").arg(Math.floor(page.models.progress * 100))
                    }
                }
            ]

            SecondaryButton {
                text: qsTr("Cancel")
                symbol: Symbols.Close
                onClicked: page.models.cancelDownload()
            }
        }

        SectionRow {
            content: [
                SearchField {
                    id: search
                    Layout.fillWidth: true
                    placeholderText: qsTr("Search models, such as Qwen or Gemma")
                    Accessible.name: qsTr("Search Hugging Face")
                    onQueryChanged: page.models.search(query)
                }
            ]
        }

        SectionRow {
            visible: page.models.searching
            title: qsTr("Searching…")
            busy: true
        }

        SectionRow {
            visible: !page.models.searching && search.query.trim().length > 0 && page.models.results.length === 0 && page.models.error.length === 0
            title: qsTr("No GGUF models match “%1”").arg(search.query.trim())
        }

        Repeater {
            model: page.models.searching ? [] : page.models.results

            ColumnLayout {
                id: result
                required property int index
                required property string modelData
                readonly property bool open: page.models.repo === modelData

                Layout.fillWidth: true
                spacing: 0

                SectionRow {
                    Layout.fillWidth: true
                    title: result.modelData
                    subtitle: qsTr("%1 downloads").arg(Number(page.models.downloads[result.index] ?? 0).toLocaleString(Qt.locale(), "f", 0))
                    chevron: true
                    disclosure: true
                    expanded: result.open
                    busy: result.open && page.models.listing
                    onClicked: page.models.openRepo(result.open ? "" : result.modelData)
                }

                Repeater {
                    model: result.open ? page.models.files : []

                    SectionRow {
                        id: file
                        required property int index
                        required property string modelData
                        readonly property real bytes: page.models.fileSizes[index] ?? 0
                        readonly property bool have: page.models.names.indexOf(modelData.replace(/\.gguf$/, "")) >= 0

                        Layout.fillWidth: true
                        Layout.leftMargin: TelamonStyle.spacingLarge * 2
                        title: modelData
                        subtitle: page.size(bytes)

                        FitBadge {
                            fit: page.fit(file.bytes)
                        }
                        SecondaryButton {
                            // One width either way, so the badges line up.
                            implicitWidth: Math.max(downloadWidth.width, downloadedWidth.width) + TelamonStyle.spacingLarge * 4
                            text: file.have ? qsTr("Downloaded") : qsTr("Download")
                            symbol: file.have ? Symbols.Check : Symbols.Download
                            enabled: !file.have && page.models.downloading.length === 0
                            onClicked: page.models.download(file.modelData)
                        }
                    }
                }
            }
        }
    }
}
