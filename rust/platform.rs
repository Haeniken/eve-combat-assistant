//! Windows API: захват области экрана, WinRT OCR и системные диалоги.

use std::path::{Path, PathBuf};

/// Возвращает рабочую область текущего монитора в физических пикселях.
/// Панель задач Windows не должна перекрывать кнопки увеличенного интерфейса.
pub fn monitor_work_area(bounds: [i32; 4]) -> [i32; 4] {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::RECT,
            Graphics::Gdi::{
                GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
            },
        };
        let rect = RECT {
            left: bounds[0],
            top: bounds[1],
            right: bounds[2],
            bottom: bounds[3],
        };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe {
            let monitor = MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST);
            if GetMonitorInfoW(monitor, &mut info) != 0 {
                let work = info.rcWork;
                return [work.left, work.top, work.right, work.bottom];
            }
        }
    }
    bounds
}
pub fn data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("EVE_COMBAT_HOME") {
        return p.into();
    }
    if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_else(|| ".".into()))
            .join("EVECombatAssistant")
    } else {
        PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap_or_else(|| {
            format!(
                "{}/.local/share",
                std::env::var("HOME").unwrap_or_else(|_| ".".into())
            )
            .into()
        }))
        .join("eve-combat-assistant")
    }
}
pub fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let a: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let b: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe {
            MoveFileExW(
                a.as_ptr(),
                b.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(from, to).map_err(|e| e.to_string())
    }
}

#[cfg(windows)]
pub fn read_screen(rect: [i32; 4]) -> Result<image::RgbaImage, String> {
    read_window_rect(std::ptr::null_mut(), rect)
}
#[cfg(windows)]
pub fn save_settings_window(path: &Path) -> Result<(), String> {
    use windows_sys::Win32::{
        Foundation::RECT,
        UI::WindowsAndMessaging::{FindWindowW, GetClientRect},
    };
    let title: Vec<u16> = "Настройки · EVE Combat Assistant"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        let window = FindWindowW(std::ptr::null(), title.as_ptr());
        if window.is_null() {
            return Err("Окно настроек ещё не создано".into());
        }
        let mut rect = RECT::default();
        if GetClientRect(window, &mut rect) == 0 {
            return Err("Размер окна недоступен".into());
        }
        read_window_rect(window, [0, 0, rect.right, rect.bottom])?
            .save(path)
            .map_err(|e| e.to_string())
    }
}
#[cfg(not(windows))]
pub fn save_settings_window(_: &Path) -> Result<(), String> {
    Err("Снимок отдельного окна поддерживается в Windows".into())
}
#[cfg(windows)]
fn read_window_rect(
    window: windows_sys::Win32::Foundation::HWND,
    rect: [i32; 4],
) -> Result<image::RgbaImage, String> {
    use std::ptr::null_mut;
    use windows_sys::Win32::Graphics::Gdi::*;
    let [x, y, w, h] = rect;
    if !(10..=8192).contains(&w) || !(10..=8192).contains(&h) {
        return Err("Неверный размер области OCR".into());
    }
    unsafe {
        let dc = GetDC(window);
        if dc.is_null() {
            return Err("Не удалось прочитать экран".into());
        }
        let memory = CreateCompatibleDC(dc);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(memory);
            ReleaseDC(window, dc);
            return Err("Не удалось прочитать экран".into());
        }
        let old = SelectObject(memory, bitmap);
        let ok = BitBlt(memory, 0, 0, w, h, dc, x, y, SRCCOPY | CAPTUREBLT) != 0;
        let mut pixels = if ok {
            std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec()
        } else {
            Vec::new()
        };
        SelectObject(memory, old);
        DeleteObject(bitmap);
        DeleteDC(memory);
        ReleaseDC(window, dc);
        if !ok {
            return Err("Не удалось прочитать экран".into());
        }
        for p in pixels.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
            p[3] = 255;
        }
        image::RgbaImage::from_raw(w as u32, h as u32, pixels)
            .ok_or("Некорректное изображение экрана".into())
    }
}
#[cfg(not(windows))]
pub fn read_screen(_: [i32; 4]) -> Result<image::RgbaImage, String> {
    Err("Автоматическое чтение экрана доступно в Windows-сборке".into())
}

#[cfg(windows)]
pub fn ocr_document(
    image: &image::RgbaImage,
    language: &str,
) -> Result<crate::ocr::Document, String> {
    use windows::{
        Globalization::Language,
        Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap},
        Media::Ocr::OcrEngine,
        Security::Cryptography::CryptographicBuffer,
        Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize},
        core::HSTRING,
    };
    thread_local! {static READY:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};
    static OCR:std::cell::RefCell<Option<(String,OcrEngine)>>=const{std::cell::RefCell::new(None)};}
    READY.with(|ready| {
        if !ready.get() {
            unsafe {
                let _ = RoInitialize(RO_INIT_MULTITHREADED);
            }
            ready.set(true);
        }
    });
    let run = || -> windows::core::Result<crate::ocr::Document> {
        let image = if image.width() > 2000 {
            image::imageops::resize(
                image,
                2000,
                (image.height() as f64 * 2000. / image.width() as f64) as u32,
                image::imageops::FilterType::Triangle,
            )
        } else {
            image::imageops::resize(
                image,
                image.width() * 2,
                image.height() * 2,
                image::imageops::FilterType::Triangle,
            )
        };
        let mut bgra = image.as_raw().clone();
        for p in bgra.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
        let buffer = CryptographicBuffer::CreateFromByteArray(&bgra)?;
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            image.width() as i32,
            image.height() as i32,
            BitmapAlphaMode::Premultiplied,
        )?;
        OCR.with(|cache| {
            let mut cache = cache.borrow_mut();
            if !cache.as_ref().is_some_and(|(lang, _)| lang == language) {
                let lang = Language::CreateLanguage(&HSTRING::from(if language == "rus" {
                    "ru-RU"
                } else {
                    "en-US"
                }))?;
                let engine = OcrEngine::TryCreateFromLanguage(&lang)
                    .or_else(|_| OcrEngine::TryCreateFromUserProfileLanguages())?;
                *cache = Some((language.into(), engine));
            }
            let result = cache.as_ref().unwrap().1.RecognizeAsync(&bitmap)?.join()?;
            let mut lines = Vec::new();
            for line in result.Lines()? {
                let mut words = Vec::new();
                for word in line.Words()? {
                    let rect = word.BoundingRect()?;
                    words.push(crate::ocr::Word {
                        text: word.Text()?.to_string(),
                        x: rect.X,
                        y: rect.Y,
                        width: rect.Width,
                        height: rect.Height,
                    });
                }
                lines.push(crate::ocr::TextLine {
                    text: line.Text()?.to_string(),
                    words,
                });
            }
            Ok(crate::ocr::Document { lines })
        })
    };
    run().map_err(|e| format!("Windows OCR недоступен: {e}"))
}
pub fn ocr_image(image: &image::RgbaImage, language: &str) -> Result<String, String> {
    Ok(ocr_document(image, language)?.text())
}
#[cfg(not(windows))]
pub fn ocr_document(_: &image::RgbaImage, _: &str) -> Result<crate::ocr::Document, String> {
    Err("Windows OCR доступен в Windows-сборке".into())
}

#[cfg(windows)]
pub fn choose_file() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut buffer = vec![0u16; 32768];
    let title: Vec<_> = std::ffi::OsStr::new("Открыть EFT / TXT / JSON")
        .encode_wide()
        .chain(Some(0))
        .collect();
    let filter: Vec<_> = "EFT / TXT / JSON\0*.eft;*.txt;*.json\0Все файлы\0*.*\0\0"
        .encode_utf16()
        .collect();
    let mut info = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFile: buffer.as_mut_ptr(),
        nMaxFile: buffer.len() as u32,
        lpstrTitle: title.as_ptr(),
        lpstrFilter: filter.as_ptr(),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if unsafe { GetOpenFileNameW(&mut info) } != 0 {
        let len = buffer.iter().position(|c| *c == 0).unwrap_or(0);
        Some(PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
    } else {
        None
    }
}
#[cfg(not(windows))]
pub fn choose_file() -> Option<PathBuf> {
    None
}

pub fn low_priority() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::*;
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}
pub fn eve_active() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::CloseHandle, System::Threading::*, UI::WindowsAndMessaging::*,
        };
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return false;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut size);
        CloseHandle(process);
        ok != 0
            && String::from_utf16_lossy(&buffer[..size as usize])
                .to_lowercase()
                .ends_with("exefile.exe")
    }
    #[cfg(not(windows))]
    {
        false
    }
}
