pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import Telamon.Ui

// One message. Yours is a bubble on the trailing side; a reply runs the
// column's width, its prose as rich text and its code in code views.
Item {
    id: message

    // The model's roles (src/chat.rs).
    required property int index
    required property string role
    required property string text
    required property list<string> kinds
    required property list<string> contents
    required property list<string> langs
    required property bool streaming
    required property bool failed

    required property var chat
    required property real columnWidth
    // The conversation's last message: a reply there can be asked again.
    required property bool last

    readonly property bool mine: role === "user"
    readonly property real pad: Kirigami.Units.largeSpacing

    // What gives a reply's rich text the Telamon look. The text itself is
    // made safe in Rust (gates-core's markdown.rs): nothing in it is raw.
    // Links in the accent, lighter on a dark theme so they read.
    readonly property color linkColor: Kirigami.Theme.backgroundColor.hslLightness < 0.5 ? Qt.lighter(TelamonStyle.accent, 1.25) : TelamonStyle.accent
    readonly property string css: "a { color: " + linkColor + "; } " + "h3 { font-size: large; } h4 { font-size: medium; } h3, h4, h5 { margin-top: 10px; margin-bottom: 4px; } " + "p { margin-top: 4px; margin-bottom: 4px; } " + "ul, ol { margin-top: 2px; margin-bottom: 2px; -qt-list-indent: 1; } li { margin-top: 2px; margin-bottom: 2px; } " + "code, pre { font-family: '" + TelamonStyle.monoFamily + "'; } " + "blockquote { margin-left: 8px; color: " + Qt.alpha(Kirigami.Theme.textColor, 0.7) + "; } " + "table { border-color: " + Qt.alpha(Kirigami.Theme.textColor, 0.25) + "; }"

    implicitHeight: column.implicitHeight

    ColumnLayout {
        id: column
        width: message.columnWidth
        anchors.horizontalCenter: parent.horizontalCenter
        spacing: Kirigami.Units.smallSpacing

        // Yours.
        Rectangle {
            visible: message.mine
            Layout.alignment: Qt.AlignRight
            implicitWidth: Math.min(mineText.implicitWidth, column.width * 0.8 - message.pad * 2) + message.pad * 2
            implicitHeight: mineText.implicitHeight + message.pad * 2
            radius: TelamonStyle.radiusLarge
            color: Qt.alpha(TelamonStyle.accent, 0.16)

            TextEdit {
                id: mineText
                x: message.pad
                y: message.pad
                width: parent.width - message.pad * 2
                readOnly: true
                selectByMouse: true
                textFormat: TextEdit.PlainText
                wrapMode: TextEdit.Wrap
                text: message.mine ? message.text : ""
                color: Kirigami.Theme.textColor
                selectionColor: TelamonStyle.selection
                selectedTextColor: Kirigami.Theme.highlightedTextColor
                font: Kirigami.Theme.defaultFont
                Accessible.role: Accessible.StaticText
                Accessible.name: qsTr("You: %1").arg(message.text)
            }
        }

        // A reply.
        RowLayout {
            visible: !message.mine
            Layout.fillWidth: true
            spacing: Kirigami.Units.largeSpacing

            Rectangle {
                Layout.alignment: Qt.AlignTop
                implicitWidth: Kirigami.Units.gridUnit * 1.6
                implicitHeight: implicitWidth
                radius: width / 2
                color: Qt.alpha(TelamonStyle.accent, 0.16)

                Symbol {
                    anchors.centerIn: parent
                    icon: Symbols.AutoAwesome
                    color: message.linkColor
                    size: Kirigami.Units.gridUnit
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing

                Repeater {
                    model: message.mine ? 0 : message.kinds.length

                    Loader {
                        id: block
                        required property int index
                        readonly property string content: message.contents[index] ?? ""
                        readonly property string lang: message.langs[index] ?? ""

                        Layout.fillWidth: true
                        sourceComponent: (message.kinds[index] ?? "") === "code" ? codeBlock : proseBlock

                        Component {
                            id: proseBlock
                            TextEdit {
                                width: block.width
                                readOnly: true
                                selectByMouse: true
                                textFormat: TextEdit.RichText
                                wrapMode: TextEdit.Wrap
                                text: "<style>" + message.css + "</style>" + block.content
                                color: Kirigami.Theme.textColor
                                selectionColor: TelamonStyle.selection
                                selectedTextColor: Kirigami.Theme.highlightedTextColor
                                font: Kirigami.Theme.defaultFont
                                // Only http, https and mailto links get here
                                // (markdown.rs drops the others).
                                onLinkActivated: link => Qt.openUrlExternally(link)

                                HoverHandler {
                                    cursorShape: parent.hoveredLink.length > 0 ? Qt.PointingHandCursor : Qt.IBeamCursor
                                }
                            }
                        }

                        Component {
                            id: codeBlock
                            ColumnLayout {
                                width: block.width
                                spacing: 2

                                TelamonLabel {
                                    visible: block.lang.length > 0
                                    textStyle: TelamonLabel.Caption
                                    textFormat: Text.PlainText
                                    text: block.lang
                                }
                                TelamonCodeView {
                                    Layout.fillWidth: true
                                    text: block.content
                                    showCopy: true
                                    Accessible.name: block.lang.length > 0 ? qsTr("%1 code").arg(block.lang) : qsTr("Code")
                                }
                            }
                        }
                    }
                }

                RowLayout {
                    visible: message.streaming && message.kinds.length === 0
                    spacing: Kirigami.Units.smallSpacing

                    TelamonSpinner {
                        implicitWidth: Kirigami.Units.gridUnit
                        implicitHeight: Kirigami.Units.gridUnit
                        animated: parent.visible
                    }
                    QQC2.Label {
                        opacity: 0.7
                        text: qsTr("Thinking…")
                    }
                }

                QQC2.Label {
                    visible: message.failed
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    color: TelamonStyle.error
                    text: qsTr("The reply stopped because of an error.")
                }

                RowLayout {
                    // A failed reply with no text still offers Regenerate.
                    visible: !message.streaming && (message.text.length > 0 || message.failed)
                    spacing: 2

                    TelamonCopyButton {
                        visible: message.text.length > 0
                        text: message.text
                    }
                    ToolbarButton {
                        visible: message.last
                        enabled: !message.chat.generating
                        symbol: Symbols.Refresh
                        text: qsTr("Regenerate")
                        onClicked: message.chat.regenerate()
                    }
                }
            }
        }
    }
}
