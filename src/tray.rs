//! System tray icon (StatusNotifierItem): click to toggle, right-click for the menu.

use crate::ipc::Cmd;
use ksni::blocking::TrayMethods;
use smithay_client_toolkit::reexports::calloop::channel::Sender;

pub struct HandTray {
    pub tx: Sender<Cmd>,
    pub active: bool,
    pub sound_on: bool,
    pub icon: Vec<ksni::Icon>,
}

impl HandTray {
    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

impl ksni::Tray for HandTray {
    fn id(&self) -> String {
        "tinyhand".into()
    }
    fn title(&self) -> String {
        if self.active { "TinyHand (on)" } else { "TinyHand" }.into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Cmd::Toggle);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            CheckmarkItem {
                label: "Pointer".into(),
                checked: self.active,
                activate: Box::new(|t: &mut Self| t.send(Cmd::Toggle)),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Tap sound".into(),
                checked: self.sound_on,
                activate: Box::new(|t: &mut Self| t.send(Cmd::Sound(None))),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Clear drawing".into(),
                activate: Box::new(|t: &mut Self| t.send(Cmd::Clear)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit TinyHand".into(),
                activate: Box::new(|t: &mut Self| t.send(Cmd::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub type Handle = ksni::blocking::Handle<HandTray>;

pub fn spawn(tray: HandTray) -> Option<Handle> {
    match tray.spawn() {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!("tinyhand: no system tray ({e}); use `tinyhand toggle` instead");
            None
        }
    }
}
