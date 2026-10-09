pragma ComponentBehavior: Bound

import QtQuick
import Telamon.Ui

// An InfoBanner that is made the first time it has something to say, and kept
// after that. A banner spends nearly all of its life hidden, and building one
// costs about a millisecond of the start-up. It takes what the app uses of an
// InfoBanner (`type`, `text`, `actions`, `closable`, `shown`, `closed`) and
// behaves like one, down to sliding in when it first appears.
Loader {
    id: lazy

    property string type: "info"
    property string text
    property list<QtObject> actions
    property bool closable: false
    property bool shown: false

    signal closed

    // The banner exists (made when `shown` first turns true; it is kept, as
    // it holds a dismissal).
    property bool made: false
    // The banner may open. False for the turn it is made in, so it slides in
    // like every later one; true at once if it is up when the window is.
    property bool ready: false

    active: lazy.made
    // Out of the layout (and its margins) while the banner is, as an
    // InfoBanner is.
    visible: lazy.item !== null && lazy.item.implicitHeight > 0

    onShownChanged: {
        if (lazy.shown) {
            lazy.made = true;
        }
    }
    Component.onCompleted: {
        if (lazy.shown) {
            lazy.made = true;
            lazy.ready = true;
        }
    }

    sourceComponent: InfoBanner {
        type: lazy.type
        text: lazy.text
        actions: lazy.actions
        closable: lazy.closable
        shown: lazy.shown && lazy.ready
        onClosed: lazy.closed()
        Component.onCompleted: Qt.callLater(() => lazy.ready = true)
    }
}
