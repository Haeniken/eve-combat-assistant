use crate::{
    engine::{Catalog, Engine},
    fits,
    model::Settings,
    ocr::{self, FrameCache, FrameDecision, Gate},
    store::Store,
    tactics,
};
use chrono::Utc;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

fn catalog() -> Catalog {
    Catalog::load().unwrap()
}
fn tid(c: &Catalog, name: &str) -> i32 {
    *c.names
        .get(&name.to_lowercase())
        .unwrap_or_else(|| panic!("Missing {name}"))
}
fn mail(c: &Catalog, km: i64, weapon: &str) -> Value {
    json!({"killmail_id":km,"killmail_time":Utc::now().to_rfc3339(),"solar_system_id":30000142,
        "victim":{"ship_type_id":587,"character_id":90000000+km,"corporation_id":98000000+km,
            "items":[{"flag":27,"item_type_id":tid(c,weapon),"quantity_destroyed":1},
                {"flag":19,"item_type_id":tid(c,"1MN Afterburner II"),"quantity_dropped":1}]},
        "attackers":[{"character_id":80000000+km}],"zkb":{"solo":true,"npc":false}})
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "eve-combat-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn percentages_use_whole_eligible_population() {
    let c = catalog();
    let mut mails = Vec::new();
    for i in 1..=4 {
        mails.push(mail(&c, i, "200mm AutoCannon II"));
    }
    for i in 5..=6 {
        mails.push(mail(&c, i, "280mm Howitzer Artillery II"));
    }
    mails.push(mail(&c, 7, "Light Neutron Blaster II"));
    let mut fleet = mail(&c, 8, "200mm AutoCannon II");
    fleet["attackers"] = json!([{"character_id":1},{"character_id":2},{"character_id":3}]);
    mails.push(fleet);
    let mut unknown = mail(&c, 9, "200mm AutoCannon II");
    unknown["victim"]["items"] = json!([]);
    mails.push(unknown);
    let settings = Settings {
        max_variants: 2,
        min_percent: 0.,
        coverage: 100.,
        ..Default::default()
    };
    let p = fits::population(&mails, &c, &settings);
    assert_eq!((p.total, p.excluded, p.unknown), (7, 1, 1));
    assert_eq!(p.variants.len(), 2);
    assert!((p.variants[0].percent.unwrap() - 400. / 7.).abs() < 1e-8);
    assert!((p.other - 100. / 7.).abs() < 1e-8);
}

#[test]
fn cargo_and_loaded_ammo_do_not_change_fit_identity() {
    let c = catalog();
    let mut original = mail(&c, 1, "200mm AutoCannon II");
    original["victim"]["items"]
        .as_array_mut()
        .unwrap()
        .push(json!({"flag":27,"item_type_id":tid(&c,"Fusion S"),"quantity_destroyed":100}));
    let a = fits::normalize(&original, &c).unwrap();
    let mut changed = original.clone();
    changed["victim"]["items"][2]["item_type_id"] = json!(tid(&c, "EMP S"));
    changed["victim"]["items"]
        .as_array_mut()
        .unwrap()
        .push(json!({"flag":5,"item_type_id":tid(&c,"Warp Scrambler II"),"quantity_destroyed":10}));
    let b = fits::normalize(&changed, &c).unwrap();
    assert_eq!(fits::fingerprint(&a), fits::fingerprint(&b));
    assert_eq!(a["modules"].as_array().unwrap().len(), 2);
    assert_ne!(a["modules"][0]["charge"], b["modules"][0]["charge"]);
}

#[test]
fn doctrine_battle_excluded_even_with_solo_labels() {
    let c = catalog();
    let mut mails: Vec<_> = (1..=3)
        .map(|i| mail(&c, i, "200mm AutoCannon II"))
        .collect();
    for m in &mut mails {
        m["victim"]["corporation_id"] = json!(98888888);
    }
    let p = fits::population(&mails, &c, &Settings::default());
    assert_eq!(p.total, 0);
    assert_eq!(p.excluded, 3);
    // Independent members of an NPC corporation are not classified as a fleet solely by corp.
    for m in &mut mails {
        m["victim"]["corporation_id"] = json!(1000044);
    }
    assert_eq!(fits::population(&mails, &c, &Settings::default()).total, 3);
}

#[test]
fn pilot_lookup_cannot_pollute_global_percentages() {
    let temp = Temp::new();
    let c = catalog();
    let mut store = Store::open(&temp.0).unwrap();
    let global = mail(&c, 1, "200mm AutoCannon II");
    let pilot = mail(&c, 2, "280mm Howitzer Artillery II");
    assert_eq!(
        store.ingest_global(std::slice::from_ref(&global)).unwrap(),
        1
    );
    assert_eq!(store.ingest_global(&[global]).unwrap(), 0);
    store.ingest(&[pilot]).unwrap();
    drop(store);
    let store = Store::open(&temp.0).unwrap();
    assert_eq!(store.global_mails(587).unwrap().len(), 1);
    assert_eq!(store.pilot_mails(587, 90000002).unwrap().len(), 1);
}

#[test]
fn pilot_fit_requires_the_selected_hull_and_recent_loss() {
    let c = catalog();
    let m = mail(&c, 1, "200mm AutoCannon II");
    let settings = Settings::default();
    assert!(fits::pilot_variant(std::slice::from_ref(&m), 90000001, 587, &c, &settings).is_some());
    assert!(fits::pilot_variant(std::slice::from_ref(&m), 90000001, 593, &c, &settings).is_none());
    let mut old = m;
    old["killmail_time"] = json!((Utc::now() - chrono::TimeDelta::days(60)).to_rfc3339());
    assert!(fits::pilot_variant(&[old], 90000001, 587, &c, &settings).is_none());
}

#[test]
fn ocr_uses_longest_exact_ship_and_handles_russian_pilots() {
    let c = catalog();
    let t = ocr::parse("Pilot: Иван Петров\nCaracal Navy Issue\n12,5 km", &c).unwrap();
    assert_eq!(t.ship, "Caracal Navy Issue");
    assert_eq!(t.pilot, "Иван Петров");
    assert_eq!(t.distance, Some(12500.));
    assert!(ocr::parse("Rifterish", &c).is_none());
}

#[test]
fn changed_target_clears_old_advice_before_confirmation() {
    let c = catalog();
    let a = ocr::parse("Rifter", &c).unwrap();
    let b = ocr::parse("Tristan", &c).unwrap();
    let mut gate = Gate::default();
    assert!(gate.feed(Some(a.clone())).is_none());
    assert_eq!(gate.feed(Some(a.clone())), Some(Some(a)));
    assert_eq!(gate.feed(Some(b.clone())), Some(None));
    assert_eq!(gate.feed(Some(b.clone())), Some(Some(b)));
    assert_eq!(gate.feed(None), Some(None));
}

#[test]
fn unchanged_frames_trigger_one_ocr_and_one_confirmation() {
    let mut cache = FrameCache::default();
    assert_eq!(cache.inspect(&[1, 2, 3]), FrameDecision::Recognize);
    assert_eq!(cache.inspect(&[1, 2, 3]), FrameDecision::Confirm);
    for _ in 0..100 {
        assert_eq!(cache.inspect(&[1, 2, 3]), FrameDecision::Ignore);
    }
    assert_eq!(cache.inspect(&[1, 2, 4]), FrameDecision::Recognize);
}

#[test]
fn collision_rigs_are_not_blaster_weapons() {
    assert_eq!(
        fits::family("Small Projectile Collision Accelerator I", 773),
        ""
    );
    assert_eq!(fits::family("Light Ion Blaster II", 74), "Blaster");
    assert_eq!(fits::family("75mm Gatling Rail II", 74), "Railgun");
}

#[test]
fn integrated_dogma_and_matchup_use_available_ammunition() {
    let c = catalog();
    let mut engine = Engine::default();
    let raw = include_str!("../resources/example-tristan.eft");
    let own = engine
        .evaluate(json!({"raw_fit":raw,"skill_level":5}))
        .unwrap();
    let enemy = engine
        .evaluate(
            json!({"raw_fit":include_str!("../resources/example-rifter.eft"),"skill_level":5}),
        )
        .unwrap();
    assert_eq!(own["hull"]["id"], 593);
    assert!(own["summary"]["offense"]["dps"].as_f64().unwrap() > 140.);
    let advice = tactics::advise(
        &own,
        &enemy,
        &mut engine,
        &c,
        &Settings::default(),
        None,
        0.,
    )
    .unwrap();
    assert!(
        advice.low > 0.
            && advice.low <= advice.high
            && advice.high
                <= own["hull"]["attributes"]["maxTargetRange"]
                    .as_f64()
                    .unwrap()
    );
    assert!(advice.ammo.contains("Spike S") || advice.ammo.contains("Caldari Navy Antimatter"));
    assert!(advice.own_dps.is_finite() && advice.own_dps > 0.);
    assert_eq!(advice.neut, "НЕТ В ФИТЕ");
    assert!(
        engine
            .evaluate(json!({"raw_fit":raw,"skill_level":6}))
            .is_err()
    );
}

#[test]
fn ocr_recognizes_selected_panel_and_overview_distance_prefix() {
    let c = catalog();
    for text in ["Haeniken\nRifter\n12.5 km", "12.5 km Haeniken Rifter"] {
        let target = crate::ocr::parse(text, &c).unwrap();
        assert_eq!(target.pilot, "Haeniken");
        assert_eq!(target.ship, "Rifter");
        assert_eq!(target.distance, Some(12500.));
    }
    assert!(
        crate::ocr::parse("Overview\nRifter", &c)
            .unwrap()
            .pilot
            .is_empty()
    );
}

#[test]
fn ocr_handles_eve_distance_grouping() {
    let catalog = catalog();
    for (text, expected) in [
        ("9,655 m", 9655.),
        ("9 655 м", 9655.),
        ("12 500 m", 12500.),
        ("12,5 км", 12500.),
    ] {
        let target =
            crate::ocr::parse(&format!("Rifter\nPilot: Haeniken\n{text}"), &catalog).unwrap();
        assert_eq!(target.distance, Some(expected));
    }
}

#[test]
fn legacy_settings_keep_the_pilot_lookup_preference() {
    let settings: Settings = serde_json::from_value(json!({"auto_sync":false})).unwrap();
    assert!(!settings.auto_pilot_lookup);
    assert!(!settings.ocr_debug);
}

#[test]
fn talwar_import_loads_ammunition_and_scripts_from_cargo() {
    let c = catalog();
    let result = Engine::default()
        .evaluate(json!({"raw_fit":include_str!("../tests/fixtures/talwar.eft")}))
        .unwrap();
    assert_eq!(result["hull"]["name"], "Talwar");
    let launchers: Vec<_> = crate::model::array(&result["modules"])
        .iter()
        .filter(|m| m["id"] == 8089)
        .collect();
    assert_eq!(launchers.len(), 7);
    assert!(
        launchers
            .iter()
            .all(|m| m["charge"]["id"] == tid(&c, "Caldari Navy Nova Light Missile"))
    );
    assert_eq!(
        crate::model::array(&result["modules"])
            .iter()
            .filter(|m| m["charge"]["name"] == "Scan Resolution Script")
            .count(),
        3
    );
    assert!(crate::model::n(&result["summary"]["offense"], "dps") > 100.);
    assert_eq!(crate::model::array(&result["cargo"]).len(), 5);
}

#[test]
fn personal_implants_and_selected_boosters_change_dogma() {
    let c = catalog();
    let mut engine = Engine::default();
    let raw = include_str!("../tests/fixtures/talwar.eft");
    let base = engine.evaluate(json!({"raw_fit":raw})).unwrap();
    let rl = tid(&c, "Zainou 'Deadeye' Rapid Launch RL-1003");
    let crash = tid(&c, "Synth Crash Booster");
    let changed = engine
        .evaluate(json!({"raw_fit":raw,"implants":[rl],"boosters":[crash]}))
        .unwrap();
    let a = crate::model::n(&base["summary"]["offense"], "dps");
    let b = crate::model::n(&changed["summary"]["offense"], "dps");
    assert!((b / a - 1. / 0.97).abs() < 1e-5, "ROF implant: {a} -> {b}");
    let cloud = |r: &Value| {
        crate::model::array(&r["modules"])
            .iter()
            .find(|m| m["id"] == 8089)
            .unwrap()["charge"]["attributes"]["aoeCloudSize"]
            .as_f64()
            .unwrap()
    };
    assert!(cloud(&changed) < cloud(&base));
    let cargo = engine
        .evaluate(json!({"raw_fit":format!("{raw}\nSynth Crash Booster x2\n")}))
        .unwrap();
    assert_eq!(cloud(&cargo), cloud(&base));
    assert!(
        engine
            .evaluate(json!({"raw_fit":raw,"implants":[rl,rl]}))
            .is_err()
    );
    assert!(
        engine
            .evaluate(json!({"raw_fit":raw,"boosters":[crash,crash]}))
            .is_err()
    );
}

#[test]
fn booster_penalties_are_applied_only_when_selected() {
    let c = catalog();
    let booster = tid(&c, "Strong Crash Booster");
    let effects = crate::model::array(&c.get(booster)["side_effects"]);
    assert!(!effects.is_empty());
    let ids: Vec<_> = effects
        .iter()
        .map(|effect| crate::model::id(effect, "id"))
        .collect();
    let raw = include_str!("../tests/fixtures/talwar.eft");
    let mut engine = Engine::default();
    let clean = engine
        .evaluate(json!({"raw_fit":raw,"boosters":[booster]}))
        .unwrap();
    let penalized = engine
        .evaluate(json!({"raw_fit":raw,"boosters":[booster],"booster_effects":ids}))
        .unwrap();
    assert_ne!(clean["hull"]["attributes"], penalized["hull"]["attributes"]);
}

#[test]
fn fitting_plan_has_real_improvements_for_talwar() {
    let c = catalog();
    let settings = Settings {
        own_fit: include_str!("../tests/fixtures/talwar.eft").into(),
        ..Default::default()
    };
    let plan = crate::fit_plan::calculate(&settings, &c).unwrap();
    assert!(
        plan.suggestions
            .iter()
            .any(|suggestion| c.name(suggestion.type_id) == "Synth Crash Booster")
    );
    assert!(plan.suggestions.iter().any(|suggestion| c.name(suggestion.type_id)=="Zainou 'Deadeye' Rapid Launch RL-1003"));
}

#[test]
fn pkce_and_callback_match_standard_and_reject_wrong_sessions() {
    assert_eq!(
        crate::account::challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
    assert_eq!(
        crate::account::callback_code("/callback?code=abc&state=expected", "expected").unwrap(),
        "abc"
    );
    for target in [
        "/callback?code=abc&state=other",
        "/callback?code=abc&state=expected&state=other",
        "/callback?state=expected",
        "/other?code=abc&state=expected",
    ] {
        assert!(crate::account::callback_code(target, "expected").is_err());
    }
}

#[test]
fn esi_fitting_import_keeps_cargo_separate() {
    let c = catalog();
    let items = json!([
        {"flag":"HiSlot0","type_id":8089,"quantity":1},
        {"flag":"HiSlot1","type_id":8089,"quantity":1},
        {"flag":"Cargo","type_id":tid(&c,"Caldari Navy Nova Light Missile"),"quantity":1000},
        {"flag":"Cargo","type_id":tid(&c,"Warp Scrambler II"),"quantity":1}
    ]);
    let eft = crate::account::items_to_eft(
        c.ship_id("Talwar").unwrap(),
        "ESI",
        crate::model::array(&items),
        &c,
    )
    .unwrap();
    let result = Engine::default().evaluate(json!({"raw_fit":eft})).unwrap();
    assert_eq!(crate::model::array(&result["modules"]).len(), 2);
    assert_eq!(crate::model::array(&result["cargo"]).len(), 2);
}

#[test]
fn forced_refresh_preserves_global_and_personal_records() {
    let temp = Temp::new();
    let c = catalog();
    let mut store = Store::open(&temp.0).unwrap();
    store
        .ingest_global(&[mail(&c, 1, "200mm AutoCannon II")])
        .unwrap();
    store.ingest(&[mail(&c, 2, "200mm AutoCannon II")]).unwrap();
    let page = "https://zkillboard.com/api/losses/shipTypeID/587/year/2026/month/10/page/1/";
    store.cache(page, &json!([]), 3600).unwrap();
    store
        .cache(
            "https://esi.evetech.net/killmails/1/hash",
            &json!({"immutable":true}),
            3600,
        )
        .unwrap();
    store.invalidate_statistics_cache().unwrap();
    assert!(store.cached(page).is_none());
    assert!(
        store
            .cached("https://esi.evetech.net/killmails/1/hash")
            .is_some()
    );
    assert_eq!(store.global_ships().unwrap(), vec![587]);
    assert_eq!(store.global_mails(587).unwrap().len(), 1);
    assert_eq!(store.pilot_mails(587, 90000002).unwrap().len(), 1);
}

#[test]
fn current_selection_matches_starred_hull_in_the_right_card_after_reordering() {
    use crate::ocr::{Document, TextLine, Word};
    let c = catalog();
    let word = |text: &str, x, y, width| Word {
        text: text.into(),
        x,
        y,
        width,
        height: 10.,
    };
    let cards = |swap: bool| {
        let (a, b) = if swap { (150., 0.) } else { (0., 150.) };
        let mut names = vec![
            word("Kurenai", a, 0., 40.),
            word("Uchiha", a + 45., 0., 40.),
            word("Vladimir", b, 0., 45.),
            word("A1", b + 50., 0., 20.),
        ];
        names.sort_by(|a, b| a.x.total_cmp(&b.x));
        let mut hulls = vec![
            word("Orca*", a + 25., 25., 35.),
            word("Thrasher*", b + 5., 25., 65.),
        ];
        hulls.sort_by(|a, b| a.x.total_cmp(&b.x));
        Document {
            lines: vec![
                TextLine {
                    text: "ignored reading order".into(),
                    words: names,
                },
                TextLine {
                    text: "ignored reading order".into(),
                    words: hulls,
                },
            ],
        }
    };
    for swap in [false, true] {
        let target = crate::ocr::selected_card(
            "Kurenai Uchiha\n38 км\nСтатус безопасности 0,0",
            &cards(swap),
            &c,
        )
        .unwrap();
        assert_eq!(target.ship, "Orca");
        assert_eq!(target.pilot, "Kurenai Uchiha");
        assert_eq!(target.distance, Some(38000.));
        assert_eq!(
            crate::ocr::selected_card("Vladimir A1\n31 km", &cards(swap), &c)
                .unwrap()
                .ship,
            "Thrasher"
        );
        assert!(crate::ocr::selected_card("Unknown Pilot\n31 km", &cards(swap), &c).is_none());
        assert!(crate::ocr::selected_card("Kurenai\n31 km", &cards(swap), &c).is_none());
    }
    let mut no_star = cards(false);
    for word in &mut no_star.lines[1].words {
        word.text = word.text.replace('*', "");
    }
    assert!(crate::ocr::selected_card("Kurenai Uchiha", &no_star, &c).is_none());
}

#[test]
fn ten_locked_targets_in_two_rows_use_the_current_pilot_in_any_position() {
    use crate::ocr::{Document, TextLine, Word};
    let c = catalog();
    for shift in 0..10 {
        let mut lines = Vec::new();
        for row in 0..2 {
            let mut names = Vec::new();
            let mut hulls = Vec::new();
            for col in 0..5 {
                let pilot = (row * 5 + col + shift) % 10;
                let x = col as f32 * 150.;
                let y = row as f32 * 100.;
                names.push(Word {
                    text: format!("Pilot{pilot}"),
                    x: x + 10.,
                    y,
                    width: 60.,
                    height: 10.,
                });
                hulls.push(Word {
                    text: if pilot == 9 { "Talwar*" } else { "Rifter*" }.into(),
                    x: x + 10.,
                    y: y + 25.,
                    width: 60.,
                    height: 10.,
                });
            }
            lines.push(TextLine {
                text: String::new(),
                words: names,
            });
            lines.push(TextLine {
                text: String::new(),
                words: hulls,
            });
        }
        let target = crate::ocr::selected_card("Pilot9\n24 km", &Document { lines }, &c).unwrap();
        assert_eq!(target.ship, "Talwar");
        assert_eq!(target.pilot, "Pilot9");
        assert_eq!(target.distance, Some(24000.));
    }
}

#[test]
fn enlarged_overlay_fits_full_hd_work_area_on_either_monitor() {
    use eframe::egui::{Rect, pos2};
    for left in [0., 1920.] {
        let work = Rect::from_min_max(pos2(left, 0.), pos2(left + 1920., 1032.));
        let (size, position) =
            crate::app::window_geometry(440., work, 1.75, Some(pos2(left + 1600., 800.)));
        let actual = Rect::from_min_size(position * 1.75, size * 1.75);
        assert!(work.contains_rect(actual));
        assert_eq!(actual.width(), 770.);
        assert!(actual.height() <= 1032.);
    }
}

#[test]
fn eft_bare_scripts_mixed_cargo_and_missing_sections_preserve_modules() {
    let c = catalog();
    let raw = include_str!("../tests/fixtures/talwar.eft");
    let mut engine = Engine::default();
    let baseline = engine.evaluate(json!({"raw_fit":raw})).unwrap();
    for text in [
        raw.replace("Scan Resolution Script x3", "Scan Resolution Script"),
        raw.lines()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
            .replace("Scan Resolution Script x3", "Scan Resolution Script"),
        format!(
            "\u{feff}  {}\r\n",
            raw.lines()
                .map(|line| format!("  {line}  "))
                .collect::<Vec<_>>()
                .join("\r\n")
        )
        .replace("Scan Resolution Script x3", "Scan Resolution Script"),
    ] {
        let result = engine.evaluate(json!({"raw_fit":text})).unwrap();
        assert_eq!(baseline["fit"]["modules"], result["fit"]["modules"]);
        assert_eq!(result["cargo"].as_array().unwrap().len(), 5);
        assert!(
            result["cargo"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["type_id"] == tid(&c, "Scan Resolution Script")
                    && item["quantity"] == 1)
        );
        assert_eq!(
            baseline["summary"]["offense"]["dps"],
            result["summary"]["offense"]["dps"]
        );
    }
    let result = engine.evaluate(json!({"raw_fit":"[Rifter, slots]\n[Empty High slot]\n200mm AutoCannon II, Fusion S /offline\n[Empty High slot]\n200mm AutoCannon II"})).unwrap();
    let modules = result["fit"]["modules"].as_array().unwrap();
    assert_eq!(modules[0]["slot"]["index"], 1);
    assert_eq!(modules[1]["slot"]["index"], 3);
    for line in [
        "Unknown Module II",
        "[Empty Strange slot]",
        "Scan Resolution Script x0",
        "Hobgoblin II x101",
        "Sensor Booster II, Missing Script",
    ] {
        assert!(
            engine
                .evaluate(json!({"raw_fit":format!("[Rifter, bad]\n{line}")}))
                .is_err(),
            "{line}"
        );
    }
}

#[test]
fn ocr_hull_correction_requires_a_unique_starred_match_and_reports_failures() {
    use crate::ocr::{Document, TextLine, Word};
    let c = catalog();
    let cards = |hull: &str| Document {
        lines: vec![
            TextLine {
                text: "Pilot9".into(),
                words: vec![Word {
                    text: "Pilot9".into(),
                    x: 10.,
                    y: 0.,
                    width: 60.,
                    height: 10.,
                }],
            },
            TextLine {
                text: hull.into(),
                words: vec![Word {
                    text: hull.into(),
                    x: 10.,
                    y: 25.,
                    width: 60.,
                    height: 10.,
                }],
            },
        ],
    };
    for (text, expected) in [
        ("Rifer*", "Rifter"),
        ("0rca★", "Orca"),
        ("Talwar∗", "Talwar"),
    ] {
        let target = ocr::identify_card("Pilot9\n24 km", &cards(text), &c).unwrap();
        assert_eq!(target.ship, expected);
        assert_eq!(target.pilot, "Pilot9");
        assert_eq!(target.distance, Some(24000.));
    }
    assert!(ocr::identify_card("Pilot9", &cards("Rifer"), &c).is_err());
    // Одинаковая дистанция до двух корпусов не разрешает выбирать первый.
    let mut ambiguous = c.clone();
    ambiguous.ships = vec![(1, "Rifter".into()), (2, "Rifler".into())].into();
    assert!(ocr::identify_card("Pilot9", &cards("Rifer*"), &ambiguous).is_err());
    assert!(
        ocr::identify_card("24 km", &cards("Rifter*"), &c)
            .unwrap_err()
            .contains("не найден ник")
    );
    assert!(
        ocr::identify_card("Another Pilot", &cards("Rifter*"), &c)
            .unwrap_err()
            .contains("не найден на карточках")
    );
    let mut duplicate = cards("Rifter*");
    duplicate.lines.push(duplicate.lines[0].clone());
    assert!(
        ocr::identify_card("Pilot9", &duplicate, &c)
            .unwrap_err()
            .contains("нескольких местах")
    );
}
