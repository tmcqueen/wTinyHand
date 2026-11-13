# TinyHand for Linux

A tiny hand on a stick that points at things on your screen. A Rust port of
[TinyHand for macOS](https://github.com/nilbuild/tinyhand) by Kamran Ahmed (MIT); the hand
art and tap sound come from that project.

Needs a Wayland compositor with `wlr-layer-shell`: KDE Plasma, Sway, Hyprland, niri, COSMIC
and friends. GNOME does not have it.

## Build and install

```sh
sudo apt install cargo build-essential pkg-config libxkbcommon-dev
./install.sh        # builds, then installs into ~/.local (no sudo)
tinyhand            # starts in the system tray
```

Or run it straight from the checkout: `cargo run --release`.

## Use

| | |
|---|---|
| Click the tray icon, or `tinyhand toggle` | Turn the pointer on or off |
| Right-click the tray icon | Menu |
| Click | Tap (nothing underneath gets clicked) |
| Hold and drag | Draw on the screen; it fades after 3 seconds |
| Hold **Ctrl** while drawing | Add an arrowhead to the end of the line |
| **Ctrl+Alt+X** | Clear the drawing |
| Double-tap **Shift**, or the arrow button in the toolbar | Click-through on or off |
| **Esc** or **Ctrl+Alt+H** | Clear everything and turn the pointer off |

### Click-through

In click-through the hand keeps following the cursor but your mouse works normally on the
apps underneath: click buttons, drag and drop, scroll, select text, type. Tapping and
drawing are off until you switch back. The toolbar still works.

Following the cursor needs KDE Plasma: TinyHand loads a small KWin script that reports
where the cursor is, and unloads it when click-through ends. On other compositors
click-through still works, but the hand is hidden and you see the normal cursor.

Wayland always draws the mouse cursor above every window, so the hand can't cover it.
Instead, on KDE, TinyHand hides the arrow while click-through is on: it switches Plasma to
a generated cursor theme (`~/.icons/tinyhand-cursors`) that inherits your own theme and
replaces only the arrow with a transparent image. The I-beam, resize handles and link hand
still show. Your theme is put back when click-through ends, when TinyHand quits or is
killed, and at the next start if it died without the chance. Apps that draw their own
cursors may keep showing the arrow. Set `"hideArrow": false` in the config to leave the
cursor theme alone.

Once you click into an app the keyboard is that app's, so double-tap Shift and Esc no
longer reach TinyHand; use the toolbar, the tray menu, or `tinyhand clickthrough off`.

The toolbar at the bottom picks a hand (the `⋯` button shows all 36) or the laser dot,
switches click-through, mutes the tap sound, or exits.

Wayland has no portable way for an app to grab a global hotkey, so to turn the pointer
*on* from the keyboard, bind a shortcut to `tinyhand toggle` in your desktop's settings
(KDE: System Settings → Keyboard → Shortcuts → Add New → Command). `install.sh` suggests
Ctrl+Alt+H to KDE through the desktop entry, which Plasma may or may not pick up.

## Automation

```sh
tinyhand on | off | toggle
tinyhand clear
tinyhand style <id>          # a hand id or `laser`; also turns the pointer on
tinyhand sound on|off|toggle
tinyhand clickthrough on|off|toggle
tinyhand status              # prints the state file
tinyhand quit
```

`tinyhand://` links from the macOS app work too: `xdg-open tinyhand://style/pink`, or
`tinyhand tinyhand://toggle`. If TinyHand isn't running, a command starts it.

While running, state is mirrored to `~/.local/share/tinyhand/state.json` (same shape as
upstream: `pid`, `active`, `style`, `soundOn`, `styles`, plus `clickThrough`). Settings live in
`~/.config/tinyhand/config.json` (`style`, `soundOn`, `pointerSize` 0.08–0.20,
`showShadow`, `fadeSeconds`; 0 keeps ink until cleared, and `hideArrow`).

## Not ported yet

Settings window, custom hands, the other 14 tap sounds, the hold-a-key interact mode,
auto-update. Outside click-through, the overlay takes the keyboard, so you can't type
into apps.

## Hacking

`src/main.rs` is the Wayland side, `src/render.rs` the tiny-skia renderer, `src/ipc.rs`
commands/state, `src/tray.rs` the tray icon, `src/cursor.rs` and `src/kwin_cursor.js` the
cursor feed for click-through, `src/arrow.rs` the arrow hiding. `tinyhand render-test out.png [style]` paints
one frame to a PNG without a compositor. `tools/import_assets.py ../tinyhand` regenerates
`assets/` from an upstream checkout (needs Pillow).

MIT. See [LICENSE](LICENSE).
