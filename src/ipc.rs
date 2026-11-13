//! Commands, the control socket, the state file and saved settings.

use crate::render::{is_style, LASER, STYLES};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Cmd {
    On,
    Off,
    Toggle,
    Clear,
    Style(String),
    Sound(Option<bool>),
    Quit,
}

/// Accepts both `style pink` and upstream's `tinyhand://style/pink`.
pub fn parse(words: &[String]) -> Result<Cmd, String> {
    let joined = words.join(" ");
    let text = joined.trim();
    let lower = text.to_lowercase();
    let body = lower.strip_prefix("tinyhand://").unwrap_or(&lower);
    let mut it = body.split(['/', ' ']).filter(|s| !s.is_empty());
    let cmd = it.next().ok_or("no command")?;
    let arg = it.next();
    match (cmd, arg) {
        ("on", _) => Ok(Cmd::On),
        ("off", _) => Ok(Cmd::Off),
        ("toggle", _) => Ok(Cmd::Toggle),
        ("clear", _) => Ok(Cmd::Clear),
        ("quit", _) => Ok(Cmd::Quit),
        ("style", Some(id)) if is_style(id) => Ok(Cmd::Style(id.to_string())),
        ("style", other) => Err(format!("unknown pointer style: {}", other.unwrap_or("none"))),
        ("sound", Some("on")) => Ok(Cmd::Sound(Some(true))),
        ("sound", Some("off")) => Ok(Cmd::Sound(Some(false))),
        ("sound", Some("toggle") | None) => Ok(Cmd::Sound(None)),
        ("sound", Some(v)) => Err(format!("unknown sound value: {v}")),
        _ => Err(format!("unknown command: {text}")),
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(fallback))
}

pub fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join("tinyhand.sock"),
        None => std::env::temp_dir().join("tinyhand.sock"),
    }
}

pub fn state_path() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("tinyhand/state.json")
}

fn config_path() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("tinyhand/config.json")
}

/// Sends a command line to a running instance. Err means nothing is listening.
pub fn send(line: &str) -> std::io::Result<String> {
    let mut stream = UnixStream::connect(socket_path())?;
    writeln!(stream, "{line}")?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    Ok(reply.trim().to_string())
}

pub fn listen(deliver: impl Fn(Cmd) + Send + 'static) -> std::io::Result<()> {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).is_err() {
                continue;
            }
            let words: Vec<String> = line.split_whitespace().map(String::from).collect();
            let reply = match parse(&words) {
                Ok(cmd) => {
                    deliver(cmd);
                    "ok".to_string()
                }
                Err(e) => format!("error: {e}"),
            };
            let _ = writeln!(&stream, "{reply}");
        }
    });
    Ok(())
}

pub struct Settings {
    pub style: String,
    pub sound_on: bool,
    pub size: f32,
    pub shadow: bool,
    pub fade_seconds: f32,
}

impl Settings {
    pub fn load() -> Settings {
        let v: serde_json::Value = std::fs::read_to_string(config_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(serde_json::Value::Null);
        Settings {
            style: v["style"].as_str().filter(|s| is_style(s)).unwrap_or("lavender").to_string(),
            sound_on: v["soundOn"].as_bool().unwrap_or(true),
            size: (v["pointerSize"].as_f64().unwrap_or(0.13) as f32).clamp(0.08, 0.20),
            shadow: v["showShadow"].as_bool().unwrap_or(true),
            fade_seconds: v["fadeSeconds"].as_f64().unwrap_or(3.0) as f32,
        }
    }

    pub fn save(&self) {
        let v = serde_json::json!({
            "style": self.style,
            "soundOn": self.sound_on,
            "pointerSize": self.size,
            "showShadow": self.shadow,
            "fadeSeconds": self.fade_seconds,
        });
        write(config_path(), &v);
    }
}

fn write(path: PathBuf, v: &serde_json::Value) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(v).unwrap_or_default() + "\n").is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

/// Same shape as upstream's state.json, for launchers and scripts.
pub fn write_state(active: bool, style: &str, sound_on: bool) {
    let mut styles: Vec<serde_json::Value> =
        STYLES.iter().map(|(id, name, _)| serde_json::json!({"id": id, "name": name, "type": "hand"})).collect();
    styles.push(serde_json::json!({"id": LASER, "name": "Laser dot", "type": "laser"}));
    let v = serde_json::json!({
        "pid": std::process::id(),
        "active": active,
        "style": style,
        "soundOn": sound_on,
        "styles": styles,
    });
    write(state_path(), &v);
}

pub fn cleanup() {
    let _ = std::fs::remove_file(state_path());
    let _ = std::fs::remove_file(socket_path());
}
