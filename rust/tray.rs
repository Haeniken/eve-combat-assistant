use crate::worker::Message;
use eframe::egui;
use std::sync::mpsc::Sender;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};
pub struct Tray {
    _icon: TrayIcon,
}
impl Tray {
    pub fn new(tx: Sender<Message>, ctx: egui::Context) -> Result<Self, String> {
        let menu = Menu::new();
        let show = MenuItem::new("Показать оверлей", true, None);
        let settings = MenuItem::new("Настройки", true, None);
        let quit = MenuItem::new("Выход", true, None);
        menu.append_items(&[&show, &settings, &quit])
            .map_err(|e| e.to_string())?;
        let (show_id, settings_id, quit_id) =
            (show.id().clone(), settings.id().clone(), quit.id().clone());
        let tx_menu = tx.clone();
        let ctx_menu = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = if event.id == show_id {
                "show"
            } else if event.id == settings_id {
                "settings"
            } else if event.id == quit_id {
                "quit"
            } else {
                return;
            };
            let _ = tx_menu.send(Message::Tray(action.into()));
            ctx_menu.request_repaint();
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                let _ = tx.send(Message::Tray("show".into()));
                ctx.request_repaint();
            }
        }));
        let mut rgba = vec![0; 32 * 32 * 4];
        for y in 0_i32..32 {
            for x in 0_i32..32 {
                let d = (x - 16).abs() + (y - 16).abs();
                if (11..=15).contains(&d) || y == 16 && (9..=23).contains(&x) {
                    let i = (y as usize * 32 + x as usize) * 4;
                    rgba[i..i + 4].copy_from_slice(&[137, 220, 239, 255]);
                }
            }
        }
        let icon = Icon::from_rgba(rgba, 32, 32).map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("EVE Combat Assistant")
            .with_icon(icon)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { _icon: tray })
    }
}
