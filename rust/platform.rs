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

// Windows OCR returns boxes in deskewed coordinates. Microsoft requires
// clockwise rotation around the image center to overlay them on source pixels:
// https://learn.microsoft.com/uwp/api/windows.media.ocr.ocrresult.textangle
#[cfg(any(windows, test))]
fn rotate_ocr_box(word: &mut crate::ocr::Word, width: f32, height: f32, degrees: f64) {
    if degrees.abs() < 0.001 || !degrees.is_finite() {
        return;
    }
    let (sin, cos) = degrees.to_radians().sin_cos();
    let (cx, cy) = (f64::from(width) * 0.5, f64::from(height) * 0.5);
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for (x, y) in [
        (word.x, word.y),
        (word.x + word.width, word.y),
        (word.x, word.y + word.height),
        (word.x + word.width, word.y + word.height),
    ] {
        let (dx, dy) = (f64::from(x) - cx, f64::from(y) - cy);
        let (x, y) = (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos);
        bounds[0] = bounds[0].min(x);
        bounds[1] = bounds[1].min(y);
        bounds[2] = bounds[2].max(x);
        bounds[3] = bounds[3].max(y);
    }
    word.x = bounds[0] as f32;
    word.y = bounds[1] as f32;
    word.width = (bounds[2] - bounds[0]) as f32;
    word.height = (bounds[3] - bounds[1]) as f32;
}
#[cfg(windows)]
fn ocr_document_with_angle(
    image: &image::RgbaImage,
    language: &str,
) -> Result<(crate::ocr::Document, Option<f64>), String> {
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
    let run = || -> windows::core::Result<(crate::ocr::Document, Option<f64>)> {
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
            let angle = result
                .TextAngle()
                .ok()
                .and_then(|a| a.Value().ok())
                .filter(|v| v.is_finite());
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
            let mut document = crate::ocr::Document { lines };
            if let Some(degrees) = angle {
                for line in &mut document.lines {
                    for word in &mut line.words {
                        rotate_ocr_box(word, image.width() as f32, image.height() as f32, degrees);
                    }
                }
            }
            Ok((document, angle))
        })
    };
    run().map_err(|e| format!("Windows OCR недоступен: {e}"))
}
pub fn ocr_document(
    image: &image::RgbaImage,
    language: &str,
) -> Result<crate::ocr::Document, String> {
    ocr_document_with_angle(image, language).map(|(document, _)| document)
}
pub fn ocr_image(image: &image::RgbaImage, language: &str) -> Result<String, String> {
    Ok(ocr_document(image, language)?.text())
}
#[cfg(not(windows))]
fn ocr_document_with_angle(
    _: &image::RgbaImage,
    _: &str,
) -> Result<(crate::ocr::Document, Option<f64>), String> {
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

pub struct DesktopCapture {
    pub rect: [i32; 4],
    pub image: image::RgbaImage,
}
/// Фон выбора области хранится только в памяти, без прозрачного fullscreen.
pub fn capture_desktop() -> Result<DesktopCapture, String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN,
        };
        let rect = unsafe {
            [
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            ]
        };
        Ok(DesktopCapture {
            rect,
            image: read_screen(rect)?,
        })
    }
    #[cfg(not(windows))]
    {
        Err("Выделение области доступно в Windows-сборке".into())
    }
}

/// Повторно читаем только строку ника. Более крупные пиксели помогают WinRT
/// различать y/u и 8/B на маленьком игровом шрифте без угадывания имени.
pub fn selected_retry_texts(
    image: &image::RgbaImage,
    document: &crate::ocr::Document,
    language: &str,
) -> Vec<crate::ocr::RetryRead> {
    let Some(pilot) = crate::ocr::selected_pilot(&document.text()) else {
        return Vec::new();
    };
    let Some(line) = document
        .lines
        .iter()
        .find(|line| crate::ocr::selected_pilot(&line.text).as_deref() == Some(&pilot))
    else {
        return Vec::new();
    };
    let scale = if image.width() > 2000 {
        2000. / image.width() as f32
    } else {
        2.
    };
    let height = line
        .words
        .iter()
        .map(|w| w.height / scale)
        .fold(0_f32, f32::max);
    if height <= 0. || !height.is_finite() {
        return Vec::new();
    }
    let left = line
        .words
        .iter()
        .map(|w| w.x / scale)
        .fold(f32::INFINITY, f32::min);
    let right = line
        .words
        .iter()
        .map(|w| (w.x + w.width) / scale)
        .fold(0_f32, f32::max);
    let top = line
        .words
        .iter()
        .map(|w| w.y / scale)
        .fold(f32::INFINITY, f32::min);
    let bottom = line
        .words
        .iter()
        .map(|w| (w.y + w.height) / scale)
        .fold(0_f32, f32::max);
    let x = (left - height).floor().clamp(0., image.width() as f32) as u32;
    let y = (top - height * 0.6)
        .floor()
        .clamp(0., image.height() as f32) as u32;
    let end_x = (right + height)
        .ceil()
        .clamp(x as f32, image.width() as f32) as u32;
    let end_y = (bottom + height * 0.6)
        .ceil()
        .clamp(y as f32, image.height() as f32) as u32;
    if end_x <= x || end_y <= y {
        return Vec::new();
    }
    let rect = [x, y, end_x - x, end_y - y];
    crop_attempts(image, rect, language, "nickname_crop")
}
/// Retry only after card matching fails. Preserve coordinates so geometry checks
/// still bind a nickname to its own hull, even after changing OCR input scale.
pub fn card_retry_documents(
    image: &image::RgbaImage,
    language: &str,
) -> Vec<(crate::ocr::RetryRead, crate::ocr::Document)> {
    let mut gray = image.clone();
    for p in gray.pixels_mut() {
        let [r, g, b, _] = p.0;
        let l = ((u32::from(r) * 3 + u32::from(g) * 6 + u32::from(b)) / 10) as u8;
        p.0 = [l, l, l, 255];
    }
    let background = image::imageops::blur(&gray, 2.);
    let mut neutral = image.clone();
    let mut local = gray.clone();
    for ((source, luma), out) in image.pixels().zip(gray.pixels()).zip(neutral.pixels_mut()) {
        let [r, g, b, _] = source.0;
        let chroma = r.max(g).max(b) - r.min(g).min(b);
        let v = if luma[0] >= 140 && chroma <= 85 {
            0
        } else {
            255
        };
        out.0 = [v, v, v, 255];
    }
    for ((out, luma), mean) in local
        .pixels_mut()
        .zip(gray.pixels())
        .zip(background.pixels())
    {
        let v = if luma[0] >= 110 && i16::from(luma[0]) - i16::from(mean[0]) >= 18 {
            0
        } else {
            255
        };
        out.0 = [v, v, v, 255];
    }
    let nearest = image::imageops::resize(
        image,
        image.width() * 2,
        image.height() * 2,
        image::imageops::FilterType::Nearest,
    );
    let scale = |width: u32| {
        if width > 2000 {
            2000. / width as f32
        } else {
            2.
        }
    };
    [
        ("cards_neutral", neutral),
        ("cards_local_contrast", local),
        ("cards_nearest_4x", nearest),
    ]
    .into_iter()
    .map(|(method, input)| {
        let factor = scale(image.width()) * image.width() as f32
            / (scale(input.width()) * input.width() as f32);
        let result = ocr_document_with_angle(&input, language);
        let (document, error, angle_degrees) = match result {
            Ok((mut doc, angle)) => {
                for line in &mut doc.lines {
                    for word in &mut line.words {
                        word.x *= factor;
                        word.y *= factor;
                        word.width *= factor;
                        word.height *= factor;
                    }
                }
                (doc, None, angle)
            }
            Err(error) => (crate::ocr::Document::default(), Some(error), None),
        };
        (
            crate::ocr::RetryRead {
                angle_degrees,
                method: method.into(),
                rect: [0, 0, image.width(), image.height()],
                text: document.text(),
                error,
            },
            document,
        )
    })
    .collect()
}
fn crop_attempts(
    image: &image::RgbaImage,
    rect: [u32; 4],
    language: &str,
    kind: &str,
) -> Vec<crate::ocr::RetryRead> {
    let [x, y, w, h] = rect;
    if w == 0 || h == 0 || x + w > image.width() || y + h > image.height() {
        return Vec::new();
    }
    let crop = image::imageops::crop_imm(image, x, y, w, h).to_image();
    let native = crop.clone();
    let enlarged =
        image::imageops::resize(&crop, w * 2, h * 2, image::imageops::FilterType::Nearest);
    let mut contrast = crop;
    for pixel in contrast.pixels_mut() {
        let [r, g, b, _] = pixel.0;
        let value = ((i32::from(r) + i32::from(g) + i32::from(b)) / 3 - 50) * 2;
        let value = value.clamp(0, 255) as u8;
        pixel.0 = [value, value, value, 255];
    }
    [("2x", native), ("4x", enlarged), ("contrast", contrast)]
        .into_iter()
        .map(
            |(method, image)| match ocr_document_with_angle(&image, language) {
                Ok((document, angle_degrees)) => crate::ocr::RetryRead {
                    angle_degrees,
                    method: format!("{kind}_{method}"),
                    rect,
                    text: document.text(),
                    error: None,
                },
                Err(error) => crate::ocr::RetryRead {
                    angle_degrees: None,
                    method: format!("{kind}_{method}"),
                    rect,
                    text: String::new(),
                    error: Some(error),
                },
            },
        )
        .collect()
}
pub fn distance_retry_texts(
    image: &image::RgbaImage,
    document: &crate::ocr::Document,
    language: &str,
) -> Vec<crate::ocr::RetryRead> {
    let Some(header) = document.lines.first() else {
        return Vec::new();
    };
    let scale = if image.width() > 2000 {
        2000. / image.width() as f32
    } else {
        2.
    };
    let height = header
        .words
        .iter()
        .map(|w| w.height / scale)
        .fold(0_f32, f32::max);
    if height <= 0. || !height.is_finite() {
        return Vec::new();
    }
    let left = header
        .words
        .iter()
        .map(|w| w.x / scale)
        .fold(f32::INFINITY, f32::min);
    let bottom = header
        .words
        .iter()
        .map(|w| (w.y + w.height) / scale)
        .fold(0_f32, f32::max);
    let x = (left - height).floor().clamp(0., image.width() as f32) as u32;
    let y = (bottom + height * 0.1)
        .floor()
        .clamp(0., image.height() as f32) as u32;
    let end = (bottom + height * 2.3)
        .ceil()
        .clamp(y as f32, image.height() as f32) as u32;
    crop_attempts(
        image,
        [x, y, image.width() - x, end - y],
        language,
        "distance_crop",
    )
}

#[cfg(test)]
mod ocr_rotation_regressions {
    use super::*;
    #[test]
    fn deskewed_word_boxes_are_rotated_clockwise_around_the_image_center() {
        let mut word = crate::ocr::Word {
            text: "Pilot".into(),
            x: 60.,
            y: 40.,
            width: 20.,
            height: 10.,
        };
        rotate_ocr_box(&mut word, 100., 100., 90.);
        for (value, expected) in [
            (word.x, 50.),
            (word.y, 60.),
            (word.width, 10.),
            (word.height, 20.),
        ] {
            assert!((value - expected).abs() < 0.001);
        }
        rotate_ocr_box(&mut word, 100., 100., -90.);
        for (value, expected) in [
            (word.x, 60.),
            (word.y, 40.),
            (word.width, 20.),
            (word.height, 10.),
        ] {
            assert!((value - expected).abs() < 0.001);
        }
    }
}
