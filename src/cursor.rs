//! Follows the cursor while click-through is on.
//!
//! In click-through the overlay takes no pointer input, so Wayland stops telling it where
//! the cursor is. On KDE a small KWin script reports the position over DBus instead.

use zbus::blocking::{connection, Connection};

const SCRIPT: &str = include_str!("kwin_cursor.js");
const PLUGIN: &str = "tinyhand-cursor";

struct Receiver(Box<dyn Fn(f32, f32) + Send + Sync>);

#[zbus::interface(name = "app.tinyhand.Cursor")]
impl Receiver {
    #[zbus(name = "Move")]
    fn move_to(&self, pos: &str) {
        if let Some((x, y)) = pos.split_once(',') {
            if let (Ok(x), Ok(y)) = (x.trim().parse(), y.trim().parse()) {
                (self.0)(x, y);
            }
        }
    }
}

pub struct Tracker {
    conn: Connection,
    loaded: bool,
}

impl Tracker {
    /// Connects to the session bus and publishes the object the KWin script calls.
    /// `deliver` gets positions in global compositor coordinates, on a DBus thread.
    pub fn new(deliver: impl Fn(f32, f32) + Send + Sync + 'static) -> Result<Tracker, String> {
        let conn = connection::Builder::session()
            .and_then(|b| b.name("app.tinyhand.TinyHand"))
            .and_then(|b| b.serve_at("/Cursor", Receiver(Box::new(deliver))))
            .and_then(|b| b.build())
            .map_err(|e| e.to_string())?;
        Ok(Tracker { conn, loaded: false })
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    fn scripting<B>(&self, method: &str, body: &B) -> zbus::Result<zbus::Message>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        self.conn.call_method(Some("org.kde.KWin"), "/Scripting", Some("org.kde.kwin.Scripting"), method, body)
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.loaded {
            return Ok(());
        }
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let path = dir.join("tinyhand-cursor.js");
        std::fs::write(&path, SCRIPT).map_err(|e| e.to_string())?;
        let path = path.to_string_lossy().into_owned();
        // A copy left behind by a crash would make loadScript refuse the name.
        let _ = self.scripting("unloadScript", &(PLUGIN,));
        let id: i32 = self
            .scripting("loadScript", &(path.as_str(), PLUGIN))
            .and_then(|m| m.body().deserialize())
            .map_err(|e| format!("KWin scripting: {e}"))?;
        if id < 0 {
            return Err("KWin refused the cursor script".into());
        }
        self.conn
            .call_method(Some("org.kde.KWin"), format!("/Scripting/Script{id}").as_str(), Some("org.kde.kwin.Script"), "run", &())
            .map_err(|e| format!("KWin scripting: {e}"))?;
        self.loaded = true;
        Ok(())
    }

    pub fn stop(&mut self) {
        if self.loaded {
            let _ = self.scripting("unloadScript", &(PLUGIN,));
            self.loaded = false;
        }
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        self.stop();
    }
}
