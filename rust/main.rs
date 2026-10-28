//! Точка входа, CLI для диагностики и запуск Windows-оверлея.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod account;
mod app;
mod diagnostics;
mod engine;
mod fit_plan;
mod fits;
mod model;
mod ocr;
mod platform;
mod settings_ui;
mod sources;
mod store;
mod style;
mod tactics;
#[cfg(windows)]
mod tray;
mod worker;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(
    version,
    about = "EVE Combat Assistant — локальный тактический оверлей"
)]
pub struct Args {
    #[arg(long)]
    demo: bool,
    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[arg(long)]
    screenshot: Option<PathBuf>,
    #[arg(long)]
    settings_screenshot: Option<PathBuf>,
    #[arg(long)]
    analyze: bool,
    #[arg(long)]
    sync: bool,
    #[arg(long, default_value = "Rifter")]
    ship: String,
    #[arg(long)]
    fit: Option<PathBuf>,
    #[arg(long, default_value_t = 90)]
    days: u32,
    #[arg(long)]
    archive_from: Option<String>,
    #[arg(long)]
    archive_to: Option<String>,
    #[arg(long)]
    ocr_test: Option<PathBuf>,
    #[arg(long, default_value = "eng")]
    ocr_language: String,
    #[arg(long, num_args = 4, allow_hyphen_values = true)]
    capture_test: Vec<i32>,
    #[arg(long, num_args = 2)]
    target_test: Vec<PathBuf>,
    #[arg(long)]
    open_settings: bool,
    #[arg(long, default_value_t = 0)]
    settings_tab: usize,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let directory = args.data_dir.clone().unwrap_or_else(|| {
        if args.demo {
            platform::data_dir().join("demo")
        } else {
            platform::data_dir()
        }
    });
    std::fs::create_dir_all(&directory)?;
    if args.target_test.len() == 2 {
        let selected_image = image::open(&args.target_test[0])?.to_rgba8();
        let selected = platform::ocr_document(&selected_image, &args.ocr_language)?;
        let cards = platform::ocr_document(
            &image::open(&args.target_test[1])?.to_rgba8(),
            &args.ocr_language,
        )?;
        let mut retries = Vec::new();
        let result = ocr::identify_capture(
            &selected_image,
            &selected,
            &cards,
            &args.ocr_language,
            &engine::Catalog::load()?,
            &mut retries,
        );
        let value = serde_json::json!({"target":result.as_ref().ok(),"result":result,"selected_text":selected.text(),"selected_retries":retries,"pilot_match":result.as_ref().ok().map(|target|ocr::pilot_match(&selected.text(),&retries,target)),"cards_text":cards.text()});
        std::fs::write(
            directory.join("target-result.json"),
            serde_json::to_vec_pretty(&value)?,
        )?;
        println!("{value}");
        return Ok(());
    }
    if !args.capture_test.is_empty() {
        let rect: [i32; 4] = args
            .capture_test
            .clone()
            .try_into()
            .map_err(|_| "Нужны 4 координаты")?;
        let image = platform::read_screen(rect)?;
        image.save(directory.join("capture.png"))?;
        let text = platform::ocr_image(&image, &args.ocr_language)?;
        std::fs::write(directory.join("ocr-result.txt"), &text)?;
        println!("{text}");
        return Ok(());
    }
    if let Some(path) = &args.ocr_test {
        let image = image::open(path)?.to_rgba8();
        let document = platform::ocr_document(&image, &args.ocr_language)?;
        let text = document.text();
        std::fs::write(
            directory.join("ocr-result.json"),
            serde_json::to_vec_pretty(&document)?,
        )?;
        let _ = std::fs::write(directory.join("ocr-result.txt"), &text);
        println!("{text}");
        return Ok(());
    }
    if args.analyze || args.sync || args.archive_from.is_some() {
        return app::cli(&args, &directory).map_err(Into::into);
    }
    let settings = model::Settings::load(&directory);
    let icon_image = image::load_from_memory(include_bytes!("../resources/icon.png"))?.to_rgba8();
    let icon = eframe::egui::IconData {
        width: icon_image.width(),
        height: icon_image.height(),
        rgba: icon_image.into_raw(),
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("EVE Combat Assistant")
            .with_icon(icon)
            .with_inner_size([settings.width, 900.])
            .with_min_inner_size([280., 200.])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top(),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "EVE Combat Assistant",
        options,
        Box::new(move |cc| Ok(Box::new(app::CombatApp::new(cc, args, directory)))),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
