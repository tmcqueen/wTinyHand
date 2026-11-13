//! Hides the ordinary arrow cursor while click-through is on, so the hand is the pointer.
//!
//! Wayland always draws the cursor above every window, so the hand can't cover it. Instead
//! we switch Plasma to a tiny cursor theme that inherits the user's own theme and replaces
//! only the arrow with a transparent image. The I-beam, resize handles and link hand still
//! come from the real theme. The previous theme is written down first, so it can be put
//! back on exit, or at the next start after a crash.

use crate::ipc;
use std::path::PathBuf;
use std::process::Command;

const THEME: &str = "tinyhand-cursors";
const ARROWS: &[&str] = &["left_ptr", "arrow", "top_left_arrow"];

fn restore_file() -> PathBuf {
    ipc::config_dir().join("cursor-restore")
}

fn read_setting(key: &str, default: &str) -> Option<String> {
    let out = Command::new("kreadconfig6")
        .args(["--file", "kcminputrc", "--group", "Mouse", "--key", key, "--default", default])
        .output()
        .ok()?;
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Some(if value.is_empty() { default.to_string() } else { value })
}

fn apply(theme: &str, size: &str) -> Result<(), String> {
    let status = Command::new("plasma-apply-cursortheme")
        .args(["--size", size, theme])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("plasma-apply-cursortheme: {e}"))?;
    status.success().then_some(()).ok_or_else(|| "plasma-apply-cursortheme failed".to_string())
}

/// An Xcursor file holding one fully transparent image per common size.
fn blank_xcursor() -> Vec<u8> {
    const IMAGE: u32 = 0xfffd_0002;
    let sizes = [24u32, 32, 48, 64, 96];
    let mut toc = Vec::new();
    let mut chunks = Vec::new();
    let mut at = 16 + 12 * sizes.len() as u32;
    for size in sizes {
        toc.extend([IMAGE, size, at]);
        // header size, type, nominal size, version, width, height, xhot, yhot, delay
        let chunk = [36, IMAGE, size, 1, size, size, 0, 0, 0];
        chunks.extend(chunk);
        chunks.extend(std::iter::repeat(0u32).take((size * size) as usize));
        at += 36 + 4 * size * size;
    }
    let mut words = vec![u32::from_le_bytes(*b"Xcur"), 16, 0x1_0000, sizes.len() as u32];
    words.extend(toc);
    words.extend(chunks);
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn write_theme(inherits: &str) -> std::io::Result<()> {
    // ~/.icons is the one per-user folder both KWin and plasma-apply-cursortheme search.
    let dir = ipc::home().join(".icons").join(THEME);
    let cursors = dir.join("cursors");
    std::fs::create_dir_all(&cursors)?;
    std::fs::write(
        dir.join("index.theme"),
        format!("[Icon Theme]\nName=TinyHand (no arrow)\nComment=Used by TinyHand while click-through is on\nInherits={inherits}\n"),
    )?;
    std::fs::write(cursors.join("default"), blank_xcursor())?;
    for name in ARROWS {
        let link = cursors.join(name);
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink("default", link)?;
    }
    Ok(())
}

pub struct ArrowHider {
    hidden: bool,
}

impl ArrowHider {
    pub fn new() -> ArrowHider {
        let mut hider = ArrowHider { hidden: restore_file().exists() };
        hider.set(false); // a previous run died with the arrow hidden
        hider
    }

    pub fn set(&mut self, hide: bool) {
        if hide == self.hidden {
            return;
        }
        let result = if hide { self.hide() } else { self.show() };
        match result {
            Ok(()) => self.hidden = hide,
            Err(e) => eprintln!("tinyhand: can't {} the arrow cursor: {e}", if hide { "hide" } else { "restore" }),
        }
    }

    fn hide(&self) -> Result<(), String> {
        let theme = read_setting("cursorTheme", "breeze_cursors").ok_or("not a Plasma session")?;
        let size = read_setting("cursorSize", "24").ok_or("not a Plasma session")?;
        if theme == THEME {
            return Err("the TinyHand cursor theme is already set".into());
        }
        write_theme(&theme).map_err(|e| e.to_string())?;
        let _ = std::fs::create_dir_all(ipc::config_dir());
        std::fs::write(restore_file(), format!("{theme}\n{size}\n")).map_err(|e| e.to_string())?;
        apply(THEME, &size).inspect_err(|_| {
            let _ = std::fs::remove_file(restore_file());
        })
    }

    fn show(&self) -> Result<(), String> {
        let saved = std::fs::read_to_string(restore_file()).map_err(|e| e.to_string())?;
        let mut lines = saved.lines();
        let theme = lines.next().filter(|t| !t.is_empty()).unwrap_or("breeze_cursors");
        let size = lines.next().filter(|s| !s.is_empty()).unwrap_or("24");
        apply(theme, size)?;
        let _ = std::fs::remove_file(restore_file());
        Ok(())
    }
}

impl Drop for ArrowHider {
    fn drop(&mut self) {
        self.set(false);
    }
}
