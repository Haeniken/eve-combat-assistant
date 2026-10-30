//! Точка входа, CLI для диагностики и запуск Windows-оверлея.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod account;
mod annual_rating;
mod annual_store;
mod app;
mod control;
mod diagnostics;
mod engine;
mod fit_metrics;
mod fit_plan;
mod fits;
mod matchups;
mod model;
mod ocr;
mod pilot;
mod pilot_lookup;
mod platform;
mod rating_scale;
mod seed;
mod settings_ui;
mod sources;
mod store;
mod style;
mod system_info;
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
    #[arg(long, hide = true)]
    prepare_seed: Option<PathBuf>,
    #[arg(long, hide = true)]
    fit_diagnostic: bool,
    #[arg(long, hide = true)]
    seed_database: Option<PathBuf>,
    #[arg(long, hide = true)]
    annual_rating_diagnostic: Option<i64>,
    #[arg(long, hide = true)]
    rebuild_pilot_model: bool,
    #[arg(long, hide = true, requires = "rebuild_pilot_model")]
    cold_pilot_model: bool,
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
    if let Err(error) = annual_rating::load_local(&directory) {
        eprintln!("SOLO: {error}");
    }
    if args.rebuild_pilot_model {
        let store = store::Store::open_with_seed(&directory, args.seed_database.as_deref())?;
        let end = args
            .archive_to
            .as_deref()
            .map(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d"))
            .transpose()?
            .unwrap_or_else(|| chrono::Utc::now().date_naive() - chrono::TimeDelta::days(1));
        let start = end - chrono::TimeDelta::days(annual_rating::window_days() - 1);
        let mut day = start;
        while day <= end {
            if store
                .annual_day_covered(&day.format("%Y%m%d").to_string())
                .is_none()
            {
                return Err(format!("Нет полного дня {day}").into());
            }
            day += chrono::TimeDelta::days(1);
        }
        let rows = store.all_annual_duels(
            start.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
            end.and_hms_opt(23, 59, 59).unwrap().and_utc().timestamp(),
        )?;
        let began = std::time::Instant::now();
        let count = annual_rating::rebuild(
            rows,
            &directory,
            end,
            &std::sync::atomic::AtomicBool::new(false),
            args.cold_pilot_model,
            &mut |iteration| eprintln!("SOLO iteration {iteration}"),
        )?;
        let value = serde_json::json!({"rated_players":count,"model":annual_rating::version(),"elapsed_ms":began.elapsed().as_millis(),"own":annual_rating::prepared(2112101671)});
        std::fs::write(
            directory.join("pilot-model-rebuild.json"),
            serde_json::to_vec_pretty(&value)?,
        )?;
        println!("{value}");
        return Ok(());
    }
    if let Some(character) = args.annual_rating_diagnostic {
        let prepared = annual_rating::prepared(character);
        let at = prepared
            .as_ref()
            .map(|e| e.checked_at)
            .unwrap_or_else(store::timestamp);
        let store = if args.seed_database.is_some() {
            store::Store::open_with_seed(&directory, args.seed_database.as_deref())?
        } else {
            store::Store::open(&directory)?
        };
        let start = std::time::Instant::now();
        let rows = store.annual_duels(character, at)?;
        let mut evidence = annual_rating::Evidence::new(character, at);
        evidence.extend_duels(&rows);
        let estimate = evidence.finish();
        let value = serde_json::json!({"prepared":prepared,"recalculated":estimate,"rows":rows.len(),"elapsed_ms":start.elapsed().as_millis()});
        std::fs::write(
            directory.join("annual-rating-diagnostic.json"),
            serde_json::to_vec_pretty(&value)?,
        )?;
        println!("{value}");
        return Ok(());
    }
    if let Some(path) = &args.prepare_seed {
        seed::prepare(path, &directory)?;
        return Ok(());
    }
    if args.fit_diagnostic {
        let mut settings = model::Settings::load(&directory);
        settings.days = args.days;
        settings.ocr_debug = true;
        if let Some(path) = &args.fit {
            settings.own_fit = std::fs::read_to_string(path)?;
        }
        let result = worker::diagnose(
            &directory,
            &args.ship,
            args.seed_database.as_deref(),
            settings,
        )?;
        std::fs::write(
            directory.join("fit-diagnostic.json"),
            serde_json::to_vec_pretty(&result)?,
        )?;
        println!("{result}");
        return Ok(());
    }
    if args.target_test.len() == 2 {
        let selected_image = image::open(&args.target_test[0])?.to_rgba8();
        let selected = platform::ocr_document(&selected_image, &args.ocr_language)?;
        let cards_image = image::open(&args.target_test[1])?.to_rgba8();
        let cards = platform::ocr_document(&cards_image, &args.ocr_language)?;
        let mut retries = Vec::new();
        let result = ocr::identify_images(
            &selected_image,
            &selected,
            &cards_image,
            &cards,
            &args.ocr_language,
            &engine::Catalog::load()?,
            &mut retries,
        );
        let value = serde_json::json!({"target":result.as_ref().ok(),"result":result,"selected_text":selected.text(),"selected_parsed":ocr::selected_text(&selected,&retries),"selected_retries":retries,"pilot_match":result.as_ref().ok().map(|target|ocr::pilot_match(&ocr::selected_text(&selected,&retries),&retries,target)),"cards_text":cards.text()});
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
