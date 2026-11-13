// Loaded into KWin by TinyHand while click-through is on. Wayland doesn't tell an app
// where the cursor is unless the cursor is over its own window, so KWin reports it.
var last = 0;
var timer = (typeof QTimer !== "undefined") ? new QTimer() : null;

function send() {
    last = Date.now();
    var p = workspace.cursorPos;
    callDBus("app.tinyhand.TinyHand", "/Cursor", "app.tinyhand.Cursor", "Move", p.x + "," + p.y);
}

if (timer) {
    timer.singleShot = true;
    timer.interval = 8;
    timer.timeout.connect(send);
}

workspace.cursorPosChanged.connect(function () {
    // At most one message per 8 ms; the timer delivers the final resting position.
    if (!timer || Date.now() - last >= 8) {
        send();
    } else if (!timer.active) {
        timer.start();
    }
});

send();
