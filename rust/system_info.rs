//! A bounded support report: no environment dump, account names or device identifiers.
use serde_json::{Value, json};

pub fn graphics(cc: &eframe::CreationContext<'_>) -> Value {
    use eframe::glow::{self, HasContext};
    let gl = cc.gl.as_ref().map(|gl| unsafe {
        json!({"backend":"OpenGL/glow", "vendor":gl.get_parameter_string(glow::VENDOR),
            "renderer":gl.get_parameter_string(glow::RENDERER), "version":gl.get_parameter_string(glow::VERSION),
            "glsl":gl.get_parameter_string(glow::SHADING_LANGUAGE_VERSION)})
    });
    let monitors: Vec<_> = cc.winit_window().map(|window| window.available_monitors().map(|monitor| {
        let size = monitor.size(); let position = monitor.position();
        json!({"width_px":size.width,"height_px":size.height,"x":position.x,"y":position.y,
            "scale_factor":monitor.scale_factor(),"refresh_millihertz":monitor.refresh_rate_millihertz()})
    }).collect()).unwrap_or_default();
    json!({"graphics":gl,"monitors":monitors})
}

fn libraries() -> Value {
    let names = [
        "eframe",
        "egui",
        "winit",
        "glow",
        "windows",
        "windows-sys",
        "ureq",
        "rustls",
        "image",
        "rusqlite",
        "libsqlite3-sys",
        "esf-dogma-engine",
    ];
    let mut libraries = serde_json::Map::new();
    for package in include_str!("../Cargo.lock").split("[[package]]").skip(1) {
        let field = |name: &str| {
            package.lines().find_map(|line| {
                line.strip_prefix(&format!("{name} = \""))?
                    .strip_suffix('"')
            })
        };
        if let (Some(name), Some(version)) = (field("name"), field("version"))
            && names.contains(&name)
        {
            libraries
                .entry(name)
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .expect("version list")
                .push(json!(version));
        }
    }
    libraries.insert("sqlite_runtime".into(), json!(rusqlite::version()));
    Value::Object(libraries)
}

pub fn collect(ui: Value) -> Value {
    json!({"schema_version":1,"time":chrono::Utc::now().to_rfc3339(),
        "application":{"name":env!("CARGO_PKG_NAME"),"version":env!("CARGO_PKG_VERSION"),
            "os":std::env::consts::OS,"architecture":std::env::consts::ARCH,"pointer_bits":usize::BITS,"debug_build":cfg!(debug_assertions)},
        "system":platform_info(),"logical_processors":std::thread::available_parallelism().ok().map(|n|n.get()),
        "libraries":libraries(),"dogma":serde_json::from_str::<Value>(include_str!("../native/upstream.json")).ok(),"ui":ui})
}

#[cfg(windows)]
fn registry_value(path: &str, name: &str, integer: bool) -> Value {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };
    let path: Vec<_> = path.encode_utf16().chain(Some(0)).collect();
    let name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
    let mut data = [0_u16; 512];
    let mut bytes = std::mem::size_of_val(&data) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            name.as_ptr(),
            if integer {
                RRF_RT_REG_DWORD
            } else {
                RRF_RT_REG_SZ
            },
            std::ptr::null_mut(),
            data.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != 0 {
        return json!({"error_code":status});
    }
    if integer {
        json!(u32::from(data[0]) | (u32::from(data[1]) << 16))
    } else {
        json!(
            String::from_utf16_lossy(
                &data[..data.iter().position(|n| *n == 0).unwrap_or(data.len())]
            )
            .trim()
        )
    }
}

#[cfg(windows)]
fn platform_info() -> Value {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let path = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let mut memory = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    let memory = if unsafe { GlobalMemoryStatusEx(&mut memory) } != 0 {
        json!({"physical_total_bytes":memory.ullTotalPhys,"physical_available_bytes":memory.ullAvailPhys,
            "commit_limit_bytes":memory.ullTotalPageFile,"commit_available_bytes":memory.ullAvailPageFile,
            "load_percent":memory.dwMemoryLoad})
    } else {
        json!({"error_code":std::io::Error::last_os_error().raw_os_error()})
    };
    let languages: windows::core::Result<Vec<String>> = {
        use windows::{
            Media::Ocr::OcrEngine,
            Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        };
        let init = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        let result = (|| {
            let languages = OcrEngine::AvailableRecognizerLanguages()?;
            languages
                .into_iter()
                .map(|language| language.LanguageTag().map(|tag| tag.to_string()))
                .collect()
        })();
        if init.is_ok() {
            unsafe { RoUninitialize() };
        }
        result
    };
    json!({"windows":{"edition":registry_value(path,"EditionID",false),
        "display_version":registry_value(path,"DisplayVersion",false),"build":registry_value(path,"CurrentBuildNumber",false),
        "revision":registry_value(path,"UBR",true)},
        "processor":registry_value(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0","ProcessorNameString",false),
        "memory":memory,"ocr_languages":languages.as_ref().ok(),"ocr_error":languages.as_ref().err().map(ToString::to_string)})
}

#[cfg(not(windows))]
fn platform_info() -> Value {
    let cpu = std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|s| {
        s.lines().find_map(|line| {
            line.strip_prefix("model name")?
                .split_once(':')
                .map(|(_, v)| v.trim().to_string())
        })
    });
    let memory = std::fs::read_to_string("/proc/meminfo").ok().map(|s| {
        s.lines()
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                ["MemTotal", "MemAvailable"]
                    .contains(&name)
                    .then(|| (name.to_string(), json!(value.trim())))
            })
            .collect::<serde_json::Map<_, _>>()
    });
    json!({"processor":cpu,"memory_kib":memory,"ocr_available":false})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_identifies_runtime_and_bundled_libraries() {
        let report = collect(json!({"graphics":null}));
        assert_eq!(report["application"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(report["libraries"]["eframe"][0], "0.37.0");
        assert_eq!(report["libraries"]["sqlite_runtime"], rusqlite::version());
        assert_eq!(report["dogma"]["sde_build"], "3503375");
        assert!(report["logical_processors"].as_u64().is_some_and(|n| n > 0));
        #[cfg(windows)]
        {
            assert!(report["system"]["windows"]["build"].is_string());
            assert!(
                report["system"]["memory"]["physical_total_bytes"]
                    .as_u64()
                    .is_some_and(|n| n > 0)
            );
            assert!(report["system"]["ocr_languages"].is_array());
        }
    }
}
