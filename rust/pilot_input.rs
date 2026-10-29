//! Explicit Windows drag/clipboard probe. No background clipboard monitoring.
use std::path::PathBuf;

#[cfg(not(windows))]
pub fn run(_: PathBuf) -> Result<(), String> {
    Err("Проверка переноса ника доступна в Windows".into())
}
#[cfg(windows)]
pub fn run(directory: PathBuf) -> Result<(), String> {
    native::run(directory)
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::diagnostics::DebugLog;
    use eframe::egui;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use serde_json::{Value, json};
    use std::sync::mpsc;
    use windows::{
        Win32::{
            Foundation::{HGLOBAL, HWND, POINTL},
            System::{
                Com::{CoTaskMemFree, DATADIR_GET, FORMATETC, IDataObject, TYMED_HGLOBAL},
                DataExchange::{
                    CloseClipboard, EnumClipboardFormats, GetClipboardData,
                    GetClipboardFormatNameW, OpenClipboard,
                },
                Memory::{GlobalLock, GlobalSize, GlobalUnlock},
                Ole::{
                    DROPEFFECT, DROPEFFECT_COPY, IDropTarget, IDropTarget_Impl, OleInitialize,
                    OleUninitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
                },
                SystemServices::MODIFIERKEYS_FLAGS,
            },
        },
        core::{Ref, Result, implement},
    };
    const LIMIT: usize = 8192;
    const FORMATS: usize = 32;
    fn format_name(id: u16) -> String {
        match id {
            1 => "CF_TEXT".into(),
            13 => "CF_UNICODETEXT".into(),
            15 => "CF_HDROP".into(),
            _ => {
                let mut name = [0u16; 256];
                let len = unsafe { GetClipboardFormatNameW(u32::from(id), &mut name) };
                if len > 0 {
                    String::from_utf16_lossy(&name[..len as usize])
                } else {
                    format!("format_{id}")
                }
            }
        }
    }
    fn reads_payload(id: u16) -> bool {
        if id == 1 || id == 13 {
            return true;
        }
        let name = format_name(id).to_lowercase();
        id >= 0xc000
            && ![
                "filecontents",
                "filegroupdescriptor",
                "filename",
                "shell idlist",
            ]
            .iter()
            .any(|kind| name.contains(kind))
    }
    fn preview(id: u16, bytes: &[u8]) -> Value {
        if id == 13 {
            let chars: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .take_while(|v| *v != 0)
                .collect();
            json!({"encoding":"UTF-16LE","text":String::from_utf16_lossy(&chars)})
        } else if id == 1
            || std::str::from_utf8(bytes).is_ok_and(|text| {
                text.chars()
                    .all(|c| !c.is_control() || ['\0', '\n', '\r', '\t'].contains(&c))
            })
        {
            json!({"encoding":"UTF-8_or_ANSI","text":String::from_utf8_lossy(bytes).trim_end_matches('\0')})
        } else {
            json!({"hex_prefix":bytes.iter().take(128).map(|b|format!("{b:02x}")).collect::<String>()})
        }
    }
    fn read_global(id: u16, handle: HGLOBAL) -> Value {
        unsafe {
            let size = GlobalSize(handle);
            let count = size.min(LIMIT);
            let pointer = GlobalLock(handle);
            if pointer.is_null() {
                return json!({"bytes":size,"error":"GlobalLock failed"});
            }
            let bytes = std::slice::from_raw_parts(pointer.cast::<u8>(), count).to_vec();
            let _ = GlobalUnlock(handle);
            json!({"bytes":size,"truncated":size>LIMIT,"preview":preview(id,&bytes)})
        }
    }
    fn inspect(data: &IDataObject, payload: bool) -> Value {
        let result = (|| -> Result<Vec<Value>> {
            unsafe {
                let formats = data.EnumFormatEtc(DATADIR_GET.0 as u32)?;
                let mut output = Vec::new();
                for _ in 0..FORMATS {
                    let mut values = [FORMATETC::default()];
                    let mut fetched = 0;
                    if formats.Next(&mut values, Some(&mut fetched)).is_err() || fetched == 0 {
                        break;
                    }
                    let mut format = values[0];
                    let mut entry = json!({"id":format.cfFormat,"name":format_name(format.cfFormat),"tymed":format.tymed});
                    if payload
                        && reads_payload(format.cfFormat)
                        && format.tymed & TYMED_HGLOBAL.0 as u32 != 0
                    {
                        format.tymed = TYMED_HGLOBAL.0 as u32;
                        match data.GetData(&format) {
                            Ok(mut medium) => {
                                if medium.tymed == TYMED_HGLOBAL.0 as u32 {
                                    entry["data"] = read_global(format.cfFormat, medium.u.hGlobal);
                                }
                                ReleaseStgMedium(&mut medium);
                            }
                            Err(error) => entry["error"] = json!(error.to_string()),
                        }
                    }
                    if !format.ptd.is_null() {
                        CoTaskMemFree(Some(format.ptd.cast()));
                    }
                    output.push(entry);
                }
                Ok(output)
            }
        })();
        match result {
            Ok(formats) => json!({"formats":formats}),
            Err(error) => json!({"error":error.to_string()}),
        }
    }
    fn clipboard() -> Value {
        unsafe {
            if let Err(error) = OpenClipboard(None) {
                return json!({"error":error.to_string()});
            }
            let mut formats = Vec::new();
            let mut previous = 0;
            for _ in 0..FORMATS {
                let id = EnumClipboardFormats(previous);
                if id == 0 {
                    break;
                }
                previous = id;
                let mut value = json!({"id":id,"name":format_name(id as u16)});
                if reads_payload(id as u16)
                    && let Ok(handle) = GetClipboardData(id)
                {
                    value["data"] = read_global(id as u16, HGLOBAL(handle.0));
                }
                formats.push(value);
            }
            let _ = CloseClipboard();
            json!({"formats":formats})
        }
    }
    #[implement(IDropTarget)]
    struct DropTarget {
        tx: mpsc::Sender<Value>,
        ctx: egui::Context,
    }
    impl DropTarget {
        fn event(&self, kind: &str, mut value: Value) {
            value["kind"] = json!(kind);
            let _ = self.tx.send(value);
            self.ctx.request_repaint();
        }
    }
    fn copy_effect(effect: *mut DROPEFFECT) {
        if !effect.is_null() {
            unsafe {
                (*effect).0 &= DROPEFFECT_COPY.0;
            }
        }
    }
    impl IDropTarget_Impl for DropTarget_Impl {
        fn DragEnter(
            &self,
            data: Ref<'_, IDataObject>,
            _: MODIFIERKEYS_FLAGS,
            _: &POINTL,
            effect: *mut DROPEFFECT,
        ) -> Result<()> {
            copy_effect(effect);
            self.event(
                "drag_enter",
                data.as_ref()
                    .map(|d| inspect(d, false))
                    .unwrap_or_else(|| json!({"error":"empty IDataObject"})),
            );
            Ok(())
        }
        fn DragOver(
            &self,
            _: MODIFIERKEYS_FLAGS,
            _: &POINTL,
            effect: *mut DROPEFFECT,
        ) -> Result<()> {
            copy_effect(effect);
            Ok(())
        }
        fn DragLeave(&self) -> Result<()> {
            self.event("drag_leave", json!({}));
            Ok(())
        }
        fn Drop(
            &self,
            data: Ref<'_, IDataObject>,
            _: MODIFIERKEYS_FLAGS,
            _: &POINTL,
            effect: *mut DROPEFFECT,
        ) -> Result<()> {
            copy_effect(effect);
            self.event(
                "drop",
                data.as_ref()
                    .map(|d| inspect(d, true))
                    .unwrap_or_else(|| json!({"error":"empty IDataObject"})),
            );
            Ok(())
        }
    }
    struct Registration {
        hwnd: HWND,
        _target: IDropTarget,
    }
    impl Registration {
        fn new(
            cc: &eframe::CreationContext<'_>,
            tx: mpsc::Sender<Value>,
        ) -> std::result::Result<Self, String> {
            let handle = cc.window_handle().map_err(|e| e.to_string())?;
            let RawWindowHandle::Win32(handle) = handle.as_raw() else {
                return Err("Нет Windows HWND".into());
            };
            let hwnd = HWND(handle.hwnd.get() as *mut _);
            unsafe {
                OleInitialize(None).map_err(|e| e.to_string())?;
            }
            let target: IDropTarget = DropTarget {
                tx,
                ctx: cc.egui_ctx.clone(),
            }
            .into();
            if let Err(error) = unsafe { RegisterDragDrop(hwnd, &target) } {
                unsafe {
                    OleUninitialize();
                }
                return Err(error.to_string());
            }
            Ok(Self {
                hwnd,
                _target: target,
            })
        }
    }
    impl Drop for Registration {
        fn drop(&mut self) {
            unsafe {
                let _ = RevokeDragDrop(self.hwnd);
                OleUninitialize();
            }
        }
    }
    struct Probe {
        rx: mpsc::Receiver<Value>,
        log: DebugLog,
        text: String,
        last: String,
        status: String,
        _registration: Option<Registration>,
    }
    impl Probe {
        fn new(cc: &eframe::CreationContext<'_>, directory: PathBuf) -> Self {
            let (tx, rx) = mpsc::channel();
            let mut log = DebugLog::pilot_input(directory);
            let result = Registration::new(cc, tx);
            let status = match &result {
                Ok(_) => "Приём перетаскивания готов".into(),
                Err(e) => format!("Ошибка приёма: {e}"),
            };
            log.event(true,json!({"kind":"probe_started","registered":result.is_ok(),"error":result.as_ref().err(),"max_formats":FORMATS,"bytes_per_format":LIMIT}));
            Self {
                rx,
                log,
                text: String::new(),
                last: String::new(),
                status,
                _registration: result.ok(),
            }
        }
        fn record(&mut self, event: Value) {
            self.status = match event["kind"].as_str() {
                Some("drop") => "Перенос получен. Данные записаны.",
                Some("paste") => "Вставка получена. Данные записаны.",
                Some("drag_enter") => "Windows передаёт объект. Отпустите кнопку мыши.",
                _ => "Перетаскивание завершено.",
            }
            .into();
            self.last = serde_json::to_string_pretty(&event).unwrap_or_default();
            self.log.event(true, event);
        }
    }
    impl eframe::App for Probe {
        fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
            while let Ok(event) = self.rx.try_recv() {
                self.record(event);
            }
            ui.heading("Проверка переноса ника");
            ui.label("Перетащите ник из локала EVE в это окно и отпустите мышь.");
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.text)
                    .desired_width(f32::INFINITY)
                    .hint_text("Ник или ссылка · Ctrl+V"),
            );
            let pasted = edit.has_focus()
                && ui.input(|input| {
                    input
                        .events
                        .iter()
                        .any(|event| matches!(event, egui::Event::Paste(_)))
                });
            if ui.button("Вставить из буфера и записать").clicked() || pasted
            {
                let mut event = clipboard();
                event["kind"] = json!("paste");
                self.record(event);
            }
            ui.label(&self.status);
            ui.small("Журнал: debug/pilot-input.jsonl. Записывается содержимое только явного переноса или вставки.");
            if let Some(error) = self.log.take_error() {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.last)
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false),
                );
            });
        }
    }
    pub fn run(directory: PathBuf) -> std::result::Result<(), String> {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("Перенос ника · EVE Combat Assistant")
                .with_inner_size([580., 360.])
                .with_always_on_top()
                .with_drag_and_drop(false),
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        };
        eframe::run_native(
            "EVE pilot input probe",
            options,
            Box::new(move |cc| Ok(Box::new(Probe::new(cc, directory)))),
        )
        .map_err(|e| e.to_string())
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use windows::Win32::{
            System::{
                Com::{DVASPECT_CONTENT, STGMEDIUM, STGMEDIUM_0},
                DataExchange::RegisterClipboardFormatW,
                Memory::{GMEM_MOVEABLE, GlobalAlloc},
            },
            UI::Shell::SHCreateDataObject,
        };
        #[test]
        fn native_data_object_probe_reads_text_and_custom_formats_without_clipboard_changes() {
            unsafe {
                OleInitialize(None).unwrap();
                let data: IDataObject =
                    SHCreateDataObject(None, None, None::<&IDataObject>).unwrap();
                for (id, bytes) in [
                    (
                        13u16,
                        "Example Pilot\0"
                            .encode_utf16()
                            .flat_map(u16::to_le_bytes)
                            .collect::<Vec<_>>(),
                    ),
                    (
                        RegisterClipboardFormatW(windows::core::w!(
                            "EVECombatAssistant.DiagnosticTest"
                        )) as u16,
                        b"showinfo:1377//123\0".to_vec(),
                    ),
                ] {
                    let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).unwrap();
                    let p = GlobalLock(handle);
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast(), bytes.len());
                    let _ = GlobalUnlock(handle);
                    let format = FORMATETC {
                        cfFormat: id,
                        dwAspect: DVASPECT_CONTENT.0,
                        lindex: -1,
                        tymed: TYMED_HGLOBAL.0 as u32,
                        ..Default::default()
                    };
                    let medium = STGMEDIUM {
                        tymed: TYMED_HGLOBAL.0 as u32,
                        u: STGMEDIUM_0 { hGlobal: handle },
                        pUnkForRelease: std::mem::ManuallyDrop::new(None),
                    };
                    data.SetData(&format, &medium, true).unwrap();
                }
                let result = inspect(&data, true);
                let formats = result["formats"].as_array().unwrap();
                assert!(
                    formats
                        .iter()
                        .any(|f| f["data"]["preview"]["text"] == "Example Pilot")
                );
                assert!(
                    formats
                        .iter()
                        .any(|f| f["name"] == "EVECombatAssistant.DiagnosticTest"
                            && f["data"]["preview"]["text"] == "showinfo:1377//123")
                );
                let metadata = inspect(&data, false);
                assert!(
                    metadata["formats"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|f| f.get("data").is_none())
                );
                let (tx, rx) = mpsc::channel();
                let target: IDropTarget = DropTarget {
                    tx,
                    ctx: egui::Context::default(),
                }
                .into();
                let mut effect = DROPEFFECT_COPY;
                target
                    .DragEnter(&data, MODIFIERKEYS_FLAGS(0), POINTL::default(), &mut effect)
                    .unwrap();
                assert_eq!(rx.recv().unwrap()["kind"], "drag_enter");
                target
                    .Drop(&data, MODIFIERKEYS_FLAGS(0), POINTL::default(), &mut effect)
                    .unwrap();
                let dropped = rx.recv().unwrap();
                assert_eq!(dropped["kind"], "drop");
                assert!(
                    dropped["formats"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|f| f["data"]["preview"]["text"] == "Example Pilot")
                );
                drop(target);
                drop(data);
                OleUninitialize();
            }
        }
    }
}
