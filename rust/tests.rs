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
fn utility_scripts_are_not_ammunition_and_unloaded_guns_try_available_charges() {
    let c = catalog();
    let mut engine = Engine::default();
    let enemy = engine
        .evaluate(json!({"raw_fit":include_str!("../resources/example-rifter.eft")}))
        .unwrap();
    for (fit, drone_only) in [
        (
            "[Vedmak, ESI missing gun]\nTracking Computer II, Tracking Speed Script\nCaldari Navy Hornet x3\nMystic M x1500",
            true,
        ),
        (
            "[Vedmak, Unloaded gun]\nHeavy Entropic Disintegrator II\nTracking Computer II, Tracking Speed Script\nMeson Exotic Plasma M x1000\nMystic M x1500\nOccult M x1000",
            false,
        ),
        (
            "[Vedmak, Loaded gun]\nHeavy Entropic Disintegrator II, Mystic M\nTracking Computer II, Tracking Speed Script",
            false,
        ),
    ] {
        let mut own = engine.evaluate(json!({"raw_fit":fit})).unwrap();
        if fit.contains("Unloaded gun") {
            let cargo = own["cargo"].clone();
            let mut unloaded = own["fit"].clone();
            for module in unloaded["modules"].as_array_mut().unwrap() {
                if c.name(crate::model::id(module, "type_id") as i32)
                    .contains("Disintegrator")
                {
                    module["charge"] = serde_json::Value::Null;
                }
            }
            own = engine.evaluate(json!({"fit":unloaded})).unwrap();
            assert_eq!(own["summary"]["offense"]["dps"].as_f64(), Some(0.));
            own["cargo"] = cargo;
        }
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
        assert!(!advice.ammo.contains("Script"));
        if drone_only {
            assert_eq!(advice.ammo, "Дроны");
            assert!(
                tactics::own_weapon_summary(&own, &c)["weapon_type_ids"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        } else {
            assert!(
                ["Meson Exotic Plasma M", "Mystic M", "Occult M"].contains(&advice.ammo.as_str()),
                "{}",
                advice.ammo
            );
            assert!(advice.own_dps > 0.);
        }
    }
}

#[test]
fn kikimora_from_supplied_fit_uses_small_plasma_and_keeps_every_module() {
    let c = catalog();
    let mut engine = Engine::default();
    let own = engine
        .evaluate(json!({"raw_fit":include_str!("../tests/fixtures/kikimora-ammo.eft")}))
        .unwrap();
    let modules = crate::model::array(&own["modules"]);
    assert_eq!(modules.len(), 12);
    assert!(
        modules
            .iter()
            .any(|m| m["name"] == "Light Entropic Disintegrator II")
    );
    assert!(own["summary"]["offense"]["dps"].as_f64().unwrap() > 50.);
    let enemy = engine
        .evaluate(json!({"raw_fit":include_str!("../resources/example-rifter.eft")}))
        .unwrap();
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
    assert!(["Mystic S", "Occult S", "Meson Exotic Plasma S"].contains(&advice.ammo.as_str()));
    let diagnostic = tactics::own_weapon_summary(&own, &c);
    assert_eq!(diagnostic["weapon_type_ids"].as_array().unwrap().len(), 1);
    assert_eq!(
        diagnostic["compatible_cargo_charge_ids"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let mut esi_items = Vec::new();
    for module in crate::model::array(&own["fit"]["modules"]) {
        let prefix = match crate::model::s(&module["slot"], "type") {
            "High" => "HiSlot",
            "Medium" => "MedSlot",
            "Low" => "LoSlot",
            "Rig" => "RigSlot",
            other => panic!("unexpected slot {other}"),
        };
        esi_items.push(json!({"flag":format!("{prefix}{}",crate::model::id(&module["slot"],"index")),"type_id":module["type_id"],"quantity":1}));
    }
    for item in crate::model::array(&own["cargo"]) {
        esi_items
            .push(json!({"flag":"Cargo","type_id":item["type_id"],"quantity":item["quantity"]}));
    }
    let eft = crate::account::items_to_eft(
        crate::model::id(&own["fit"], "ship_type_id") as i32,
        "ESI roundtrip",
        &esi_items,
        &c,
    )
    .unwrap();
    let imported = engine.evaluate(json!({"raw_fit":eft})).unwrap();
    assert_eq!(crate::model::array(&imported["modules"]).len(), 12);
    assert!(
        crate::model::array(&imported["modules"])
            .iter()
            .any(|m| m["name"] == "Light Entropic Disintegrator II")
    );
}

#[test]
fn boarding_another_ship_replaces_or_clears_fit_but_same_ship_preserves_manual_eft() {
    use crate::account::Profile;
    let previous = Profile {
        character_id: 7,
        ship: json!({"ship_item_id":100,"ship_type_id":52265}),
        ..Default::default()
    };
    let mut current = previous.clone();
    current.current_fit = Some("ESI partial snapshot".into());
    let mut selected = "Full manual Kikimora EFT".to_string();
    current.apply_current_fit(&previous, &mut selected);
    assert_eq!(selected, "Full manual Kikimora EFT");
    // Two hulls of the same type can have different fits: identity is the item.
    current.ship["ship_item_id"] = json!(101);
    current.current_fit = Some("Other Kikimora EFT".into());
    assert!(crate::account::ship_changed(100, &current.ship));
    current.apply_current_fit(&previous, &mut selected);
    assert_eq!(selected, "Other Kikimora EFT");
    current.current_fit = None;
    current.apply_current_fit(&previous, &mut selected);
    assert!(
        selected.is_empty(),
        "never keep the previous ship's fitting"
    );
    assert!(!crate::account::ship_changed(
        100,
        &json!({"ship_item_id":100})
    ));
    assert!(!crate::account::ship_changed(100, &json!({})));
}

#[test]
fn enemy_fitted_speed_separates_base_afterburner_and_mwd() {
    let c = catalog();
    let mut engine = Engine::default();
    let bare = engine
        .evaluate(json!({"raw_fit":"[Gnosis, Bare]"}))
        .unwrap();
    for (propulsion, ab, mwd) in [
        ("", false, false),
        ("10MN Afterburner II", true, false),
        ("50MN Microwarpdrive II", false, true),
        ("10MN Afterburner II\n50MN Microwarpdrive II", true, true),
    ] {
        let result = engine.evaluate(json!({"raw_fit":format!("[Gnosis, Speed]\nNanofiber Internal Structure II\n{propulsion}")})).unwrap();
        let speed = crate::fit_metrics::fitted_speed(&result, &mut engine, &c, None).unwrap();
        let base = crate::model::n(&speed, "base_mps");
        assert!(
            base > bare["hull"]["attributes"]["maxVelocity"].as_f64().unwrap(),
            "base must retain fitted speed bonuses"
        );
        assert_eq!(speed.get("ab_mps").is_some(), ab);
        assert_eq!(speed.get("mwd_mps").is_some(), mwd);
        for key in ["ab_mps", "mwd_mps"] {
            if let Some(v) = speed[key].as_f64() {
                assert!(v > base);
            }
        }
        if ab && mwd {
            let mut off = result["fit"].clone();
            for module in off["modules"].as_array_mut().unwrap() {
                if c.name(crate::model::id(module, "type_id") as i32)
                    .contains("Afterburner")
                {
                    module["state"] = json!("Online");
                }
            }
            let only_mwd = engine.evaluate(json!({"fit":off})).unwrap();
            assert!(
                (crate::model::n(&speed, "mwd_mps")
                    - only_mwd["hull"]["attributes"]["maxVelocity"]
                        .as_f64()
                        .unwrap())
                .abs()
                    < 0.01
            );
        }
        assert!(speed.get("ab_heat_mps").is_none());
        assert!(speed.get("mwd_heat_mps").is_none());
    }
}

#[test]
fn own_speed_uses_real_skill_map_instead_of_enemy_v() {
    let c = catalog();
    let mut engine = Engine::default();
    let raw = "[Gnosis, Dual]\nNanofiber Internal Structure II\n10MN Afterburner II\n50MN Microwarpdrive II";
    let settings = Settings {
        skills: Some(json!({(tid(&c,"Navigation").to_string()):2})),
        ..Settings::default()
    };
    // Unspecified skills are zero; the actual Navigation II map must survive propulsion trials.
    let own = engine
        .evaluate(settings.own_request(json!(raw), true))
        .unwrap();
    let own_speed =
        crate::fit_metrics::fitted_speed(&own, &mut engine, &c, Some(&settings)).unwrap();
    let enemy = engine
        .evaluate(json!({"raw_fit":raw,"skill_level":5}))
        .unwrap();
    let enemy_speed = crate::fit_metrics::fitted_speed(&enemy, &mut engine, &c, None).unwrap();
    for key in ["base_mps", "ab_mps", "mwd_mps"] {
        assert!(
            crate::model::n(&enemy_speed, key) > crate::model::n(&own_speed, key),
            "skills must affect {key}"
        );
    }
    let mut fit = own["fit"].clone();
    for m in fit["modules"].as_array_mut().unwrap() {
        let name = c.name(crate::model::id(m, "type_id") as i32);
        if name.contains("Afterburner") {
            m["state"] = json!("Online");
        }
        if name.contains("Microwarpdrive") {
            m["state"] = json!("Active");
        }
    }
    let active = engine.evaluate(settings.own_request(fit, false)).unwrap();
    assert!(
        (crate::model::n(&own_speed, "mwd_mps")
            - crate::model::n(&active["hull"]["attributes"], "maxVelocity"))
        .abs()
            < 0.001
    );
    let mixed = crate::fit_metrics::speed_rows(
        &json!({"base_mps":175,"ab_mps":700}),
        &json!({"base_mps":250,"mwd_mps":1700}),
    );
    assert_eq!(mixed.len(), 2);
    assert_eq!(mixed[1].label, "AB/MWD");
    assert_eq!(mixed[1].own, Some(1700.));
    assert!(
        crate::fit_metrics::speed_rows(&enemy_speed, &Value::Null)
            .iter()
            .all(|r| r.own.is_none())
    );
}

#[test]
fn weapon_reach_selects_legal_long_ammo_and_keeps_fit_bonuses() {
    let c = catalog();
    let mut engine = Engine::default();
    for (gun, charge) in [
        ("125mm Railgun I", "Iron Charge S"),
        ("125mm Railgun II", "Spike S"),
        ("200mm AutoCannon II", "Barrage S"),
    ] {
        let r=engine.evaluate(json!({"raw_fit":format!("[Rifter, Range]\n{gun}\nTracking Computer II, Optimal Range Script")})).unwrap();
        let range = crate::fit_metrics::weapon_range(&r, &mut engine, &c);
        assert!(range["errors"].as_array().unwrap().is_empty(), "{range}");
        let source = &range["sources"][0];
        assert!(
            source["charge"].as_str().unwrap().ends_with(charge),
            "{source}"
        );
        assert_eq!(source["method"], "optimal_plus_2_falloff");
        assert!(
            (crate::model::n(source, "max_range_m")
                - crate::model::n(source, "optimal_m")
                - 2. * crate::model::n(source, "falloff_m"))
            .abs()
                < 0.001
        );
        let plain = engine
            .evaluate(json!({"raw_fit":format!("[Rifter, Range]\n{gun}, {charge}")}))
            .unwrap();
        let actual = plain["modules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["slot"] == "high")
            .unwrap();
        let plain_reach = crate::model::n(&actual["attributes"], "maxRange")
            + 2. * crate::model::n(&actual["attributes"], "falloff");
        assert!(
            crate::model::n(&range, "max_range_m") > plain_reach,
            "range modules must count"
        );
    }
    let kiki = engine
        .evaluate(json!({"raw_fit":include_str!("../tests/fixtures/kikimora-ammo.eft")}))
        .unwrap();
    let reach = crate::fit_metrics::weapon_range(&kiki, &mut engine, &c);
    assert_eq!(reach["sources"][0]["charge"], "Meson Exotic Plasma S");
    assert_eq!(reach["approximate"], false);
    assert_eq!(reach["sources"][0]["falloff_m"], 0.);
    assert_eq!(
        reach["sources"].as_array().unwrap().len(),
        1,
        "utility modules must not become weapons"
    );
    let utility=engine.evaluate(json!({"raw_fit":"[Tristan, Utility]\nAuto Targeting System I\nTracking Computer II, Tracking Speed Script\n\nHobgoblin II x5"})).unwrap();
    let reach = crate::fit_metrics::weapon_range(&utility, &mut engine, &c);
    assert!(
        reach["max_range_m"].is_null() && reach["sources"].as_array().unwrap().is_empty(),
        "no drone or utility reach"
    );
}

#[test]
fn speed_is_not_zeroed_by_automatically_active_cyno_and_jump_portal() {
    let c = catalog();
    let mut engine = Engine::default();
    let req: Value =
        serde_json::from_str(include_str!("../resources/fixtures/marshal-0.json")).unwrap();
    let result = engine.evaluate(req).unwrap();
    let original = result["fit"].clone();
    assert_eq!(
        crate::model::n(&result["hull"]["attributes"], "maxVelocity"),
        0.
    );
    let speed = crate::fit_metrics::fitted_speed(&result, &mut engine, &c, None).unwrap();
    assert!(crate::model::n(&speed, "base_mps") > 100.);
    assert!(crate::model::n(&speed, "ab_mps") > crate::model::n(&speed, "base_mps"));
    assert!(crate::model::n(&speed, "mwd_mps") > crate::model::n(&speed, "ab_mps"));
    assert!(speed["inactive_movement_modules"].as_array().unwrap().len() >= 2);
    assert_eq!(
        original, result["fit"],
        "movement scenario must not change imported fitting"
    );
}

#[test]
fn tactical_destroyer_speed_defaults_to_sde_propulsion_mode_on_both_sides() {
    let c = catalog();
    let mut engine = Engine::default();
    let own_settings = Settings {
        skills: Some(json!({(tid(&c,"Navigation").to_string()):2})),
        ..Settings::default()
    };
    for ship in ["Confessor", "Svipul", "Jackdaw", "Hecate"] {
        let raw = format!(
            "[{ship}, Mode]\nNanofiber Internal Structure II\n1MN Afterburner II\n5MN Microwarpdrive II"
        );
        let result = engine.evaluate(json!({"raw_fit":raw})).unwrap();
        let original = result["fit"].clone();
        let mode = crate::engine::propulsion_mode(tid(&c, ship))
            .unwrap()
            .unwrap();
        let mut explicit = original.clone();
        explicit["ship_mode"] = mode["id"].clone();
        for m in explicit["modules"].as_array_mut().unwrap() {
            let name = c.name(crate::model::id(m, "type_id") as i32);
            if name.contains("Afterburner") || name.contains("Microwarpdrive") {
                m["state"] = json!("Online");
            }
        }
        let expected = engine.evaluate(json!({"fit":explicit})).unwrap();
        let speed = crate::fit_metrics::fitted_speed(&result, &mut engine, &c, None).unwrap();
        assert_eq!(speed["ship_mode"]["id"], mode["id"]);
        assert!(
            (crate::model::n(&speed, "base_mps")
                - crate::model::n(&expected["hull"]["attributes"], "maxVelocity"))
            .abs()
                < 0.001
        );
        let mut neutral = explicit.clone();
        neutral["ship_mode"] = Value::Null;
        let neutral = engine.evaluate(json!({"fit":neutral})).unwrap();
        assert!(
            crate::model::n(&speed, "base_mps")
                >= crate::model::n(&neutral["hull"]["attributes"], "maxVelocity")
        );
        let mut no_mode = explicit.clone();
        no_mode["ship_mode"] = Value::Null;
        for m in no_mode["modules"].as_array_mut().unwrap() {
            if c.name(crate::model::id(m, "type_id") as i32)
                .contains("Microwarpdrive")
            {
                m["state"] = json!("Active");
            }
        }
        let ordinary = engine.evaluate(json!({"fit":no_mode})).unwrap();
        assert!(
            crate::model::n(&speed, "mwd_mps")
                > crate::model::n(&ordinary["hull"]["attributes"], "maxVelocity"),
            "{ship} must receive the real propulsion-mode bonus"
        );

        let own = engine
            .evaluate(own_settings.own_request(json!(raw), true))
            .unwrap();
        let own_speed =
            crate::fit_metrics::fitted_speed(&own, &mut engine, &c, Some(&own_settings)).unwrap();
        assert_eq!(own_speed["ship_mode"]["id"], mode["id"]);
        assert!(crate::model::n(&speed, "base_mps") > crate::model::n(&own_speed, "base_mps"));
        assert_eq!(result["fit"], original);
        let invalid = engine.evaluate(
            json!({"fit":{"ship_type_id":587,"ship_mode":mode["id"],"modules":[],"drones":[]}}),
        );
        assert!(invalid.is_err(), "cannot apply another hull's mode");
    }
}

#[test]
fn neutralizer_count_and_size_do_not_assign_long_reach_to_short_modules() {
    let mut engine = Engine::default();
    let fit=engine.evaluate(json!({"raw_fit":"[Armageddon, Neuts]\nSmall Energy Neutralizer II\nSmall Energy Neutralizer II\nMedium Energy Neutralizer II\nHeavy Energy Neutralizer II"})).unwrap();
    let neuts = crate::fit_metrics::neutralizers(&fit);
    let groups = neuts["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 3);
    let light = groups.iter().find(|g| g["size"] == "Light").unwrap();
    assert_eq!(light["count"], 2);
    assert_eq!(groups[0]["size"], "Heavy");
    assert!(crate::model::n(&groups[0], "range_m") > crate::model::n(light, "range_m"));
    assert!(neuts["label"].as_str().unwrap().contains("2× Light"));
    assert!(neuts["label"].as_str().unwrap().contains("1× Medium"));
}

#[test]
fn wrapped_nickname_parts_must_belong_to_one_card_column() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../resources/fixtures/ocr-background-cards.json"
    ))
    .unwrap();
    let row = fixture.as_array().unwrap().last().unwrap();
    let mut cards: ocr::Document = serde_json::from_value(row["cards"].clone()).unwrap();
    let c = catalog();
    assert_eq!(
        ocr::identify_card("Juborun Vemane", &cards, &c)
            .unwrap()
            .ship,
        "Brutix Navy Issue"
    );
    for word in &mut cards.lines[1].words {
        word.x += 300.;
    }
    assert!(ocr::identify_card("Juborun Vemane", &cards, &c).is_err());
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
fn esi_fitting_import_resolves_non_fitting_cargo_and_preserves_type_ids() {
    let c = catalog();
    // Nanite paste, deployables and filaments occur in real ESI cargo but are
    // intentionally absent from the small combat-type catalog.
    let cargo = [28668, 28672, 45998, 56076];
    let mut items = vec![json!({"flag":"HiSlot0","type_id":8089,"quantity":1})];
    for type_id in cargo {
        items.push(json!({"flag":"Cargo","type_id":type_id,"quantity":2}));
    }
    let eft = crate::account::items_to_eft(32878, "ESI cargo", &items, &c).unwrap();
    assert!(!eft.contains("Неизвестный модуль"));
    let result = Engine::default().evaluate(json!({"raw_fit":eft})).unwrap();
    assert_eq!(crate::model::array(&result["modules"]).len(), 1);
    for type_id in cargo {
        assert!(crate::model::array(&result["cargo"]).iter().any(|item| {
            crate::model::id(item, "type_id") == type_id && crate::model::id(item, "quantity") == 2
        }));
    }
    let unknown = crate::account::items_to_eft(
        32878,
        "Unknown ESI item",
        &[json!({"flag":"Cargo","type_id":2147483647,"quantity":1})],
        &c,
    )
    .unwrap_err();
    assert!(unknown.contains("type_id=2147483647"));
    let legacy = Engine::default()
        .evaluate(json!({"raw_fit":"[Talwar, Old ESI snapshot]\nНеизвестный модуль x4"}))
        .unwrap_err();
    assert!(legacy.contains("Обновите персонажа"));
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
    assert_eq!(
        crate::ocr::selected_card("Kurenai Uchiha", &no_star, &c)
            .unwrap()
            .ship,
        "Orca"
    );
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

#[test]
fn screenshot_selection_maps_physical_pixels_across_monitors_and_zoom() {
    use eframe::egui::{Rect, pos2};
    for zoom in [0.75, 1., 1.25, 1.75] {
        let canvas = Rect::from_min_max(pos2(0., 0.), pos2(3840. / zoom, 1080. / zoom));
        let selection = Rect::from_min_max(
            pos2(2100. / zoom, 20. / zoom),
            pos2(2500. / zoom, 150. / zoom),
        );
        assert_eq!(
            crate::app::selection_rectangle(selection, canvas, [-1920, 0, 3840, 1080]),
            Some([180, 20, 400, 130])
        );
    }
    let canvas = Rect::from_min_max(pos2(0., 0.), pos2(1920., 1080.));
    assert_eq!(
        crate::app::selection_rectangle(
            Rect::from_min_max(pos2(-20., -10.), pos2(100., 80.)),
            canvas,
            [-1920, 0, 1920, 1080]
        ),
        Some([-1920, 0, 100, 80])
    );
    assert!(
        crate::app::selection_rectangle(
            Rect::from_min_max(pos2(2., 2.), pos2(8., 8.)),
            canvas,
            [0, 0, 1920, 1080]
        )
        .is_none()
    );
}

#[test]
fn nickname_matching_accepts_unique_closest_card_and_rejects_ties() {
    use crate::ocr::{Document, RetryRead, TextLine, Word};
    let c = catalog();
    let cards = |names: &[&str]| {
        let mut lines = Vec::new();
        for (index, pilot) in names.iter().enumerate() {
            let x = index as f32 * 200.;
            for (y, text) in [(0., *pilot), (25., "Coercer*")] {
                lines.push(TextLine {
                    text: text.into(),
                    words: vec![Word {
                        text: text.into(),
                        x,
                        y,
                        width: 100.,
                        height: 10.,
                    }],
                });
            }
        }
        Document { lines }
    };
    let selected = "Kennedu 518\n6 236 M";
    let retry = |text: &str| RetryRead {
        angle_degrees: None,
        method: "nickname_crop_4x".into(),
        rect: [120, 8, 110, 32],
        text: text.into(),
        error: None,
    };
    let single = cards(&["Kennedy 518"]);
    let closest = ocr::identify_with_retries(selected, &[], &single, &c).unwrap();
    assert_eq!(closest.pilot, "Kennedy 518");
    assert_eq!(closest.ship, "Coercer");
    assert_eq!(closest.distance, Some(6236.));
    assert_eq!(ocr::pilot_match(selected, &[], &closest).method, "closest");
    assert_eq!(ocr::pilot_match(selected, &[], &closest).edit_distance, 1);
    let multiple = cards(&["Kennedy 518", "Kennedy 51B"]);
    for retries in [
        vec![],
        vec![retry("Kennedy 519")],
        vec![retry("Kennedy 518")],
        vec![retry("Kennedy 518"), retry("Kennedy 51B")],
    ] {
        assert_eq!(
            ocr::identify_with_retries(selected, &retries, &multiple, &c)
                .unwrap()
                .pilot,
            "Kennedy 518"
        );
    }
    let equal = cards(&["Kennedy 518", "Kennedv 518"]);
    assert!(
        ocr::identify_with_retries(selected, &[], &equal, &c)
            .unwrap_err()
            .contains("одинаково")
    );
    assert!(
        ocr::identify_with_retries(
            selected,
            &[retry("Kennedy 518"), retry("Kennedv 518")],
            &equal,
            &c
        )
        .is_err()
    );
    assert!(ocr::identify_with_retries("Unrelated Pilot", &[], &single, &c).is_err());
    let mut no_star = single.clone();
    no_star.lines[1].words[0].text = "Coercer".into();
    assert!(ocr::identify_with_retries(selected, &[], &no_star, &c).is_err());
    for placeholder in [
        "Объект не выбран",
        "06beKT He Bb16paH",
        "No object selected",
    ] {
        assert!(ocr::selected_pilot(placeholder).is_none());
        assert!(
            ocr::identify_with_retries(placeholder, &[retry("Kennedy 518")], &single, &c)
                .unwrap_err()
                .contains("не выбран объект")
        );
    }
}

#[test]
fn selected_panel_separates_arbitrary_titles_and_ignores_security_ocr() {
    use crate::ocr::{Document, TextLine, Word};
    let word = |text: &str, x, y, width, height| Word {
        text: text.into(),
        x,
        y,
        width,
        height,
    };
    for title in ["HauraTop", "Навигатор", "Неизвестное звание"] {
        let raw = Document {
            lines: vec![
                TextLine {
                    text: format!("Ke•on Ishaq {title}"),
                    words: vec![
                        word("Ke•on", 269., 27., 59., 22.),
                        word("Ishaq", 341., 27., 61., 23.),
                        word(title, 428., 27., 100., 18.),
                    ],
                },
                TextLine {
                    text: "CTaryc 6eaonacH0ffM".into(),
                    words: vec![
                        word("CTaryc", 271., 103., 69., 22.),
                        word("6eaonacH0ffM", 351., 101., 153., 19.),
                    ],
                },
            ],
        };
        let parsed = ocr::selected_document(&raw);
        assert_eq!(parsed.lines[0].text, "Ke'on Ishaq");
        assert_eq!(parsed.lines[0].words.len(), 2);
        let retries = vec![ocr::RetryRead {
            angle_degrees: None,
            method: "distance_crop_4x".into(),
            rect: [123, 26, 229, 26],
            text: "10 KM".into(),
            error: None,
        }];
        let text = ocr::selected_text(&raw, &retries);
        assert_eq!(ocr::selected_pilot(&text).as_deref(), Some("Ke'on Ishaq"));
        assert_eq!(ocr::read_distance(&text), Some(10000.));
        assert!(!text.contains(title));
    }
    for status in [
        "Статус безопасности 0,7",
        "Статусбезопасности -1,2",
        "CTaryc 6eaonacH0ffM",
        "Craryc beaonacHocnq -1,2",
        "Security status 0.7",
    ] {
        assert!(ocr::selected_pilot(status).is_none(), "{status}");
        assert_eq!(ocr::read_distance(status), None);
    }
    assert_eq!(
        ocr::selected_pilot("Black Council\n8 418 M\nCraryc beaonacHocnq -1,2").as_deref(),
        Some("Black Council")
    );
}

#[test]
fn distance_supports_cyrillic_units_grouping_and_ocr_digits() {
    for (text, meters) in [
        ("10 км", 10000.),
        ("3 021 м", 3021.),
        ("8 418 M", 8418.),
        ("lø KM", 10000.),
        ("10км", 10000.),
        ("3\u{202f}021м", 3021.),
        ("12,5 км", 12500.),
        ("1 083 km", 1083000.),
    ] {
        assert_eq!(ocr::read_distance(text), Some(meters), "{text}");
    }
}

#[test]
fn catalog_hull_matching_accepts_ocr_star_and_name_errors() {
    use crate::ocr::{Document, TextLine, Word};
    let c = catalog();
    for (read, expected) in [
        ("Machariet•", "Machariel"),
        ("Machariel·", "Machariel"),
        ("Kikimora*", "Kikimora"),
        ("Kikimara•", "Kikimora"),
    ] {
        let cards = Document {
            lines: vec![
                TextLine {
                    text: "Black Council".into(),
                    words: vec![Word {
                        text: "Black Council".into(),
                        x: 943.,
                        y: 185.,
                        width: 147.,
                        height: 18.,
                    }],
                },
                TextLine {
                    text: read.into(),
                    words: vec![Word {
                        text: read.into(),
                        x: 957.,
                        y: 243.,
                        width: 118.,
                        height: 17.,
                    }],
                },
            ],
        };
        let target = ocr::identify_card("Black Council\n8 418 M", &cards, &c).unwrap();
        assert_eq!(target.ship, expected);
        assert_eq!(target.distance, Some(8418.));
    }
}

#[test]
fn inline_corporation_tag_does_not_hide_ship_name_selection() {
    use crate::ocr::{Document, TextLine, Word};
    let c = catalog();
    let word = |text: &str, x, y, width, height| Word {
        text: text.into(),
        x,
        y,
        width,
        height,
    };
    let selected = Document {
        lines: vec![TextLine {
            text: "Velator•".into(),
            words: vec![word("Velator•", 147., 30., 91., 21.)],
        }],
    };
    let text = ocr::selected_document(&selected).text();
    assert_eq!(text, "Velator");
    let cards = Document {
        lines: vec![
            TextLine {
                text: "Velator [HDEUJ]".into(),
                words: vec![
                    word("Velator", 923., 185., 79., 18.),
                    word("[HDEUJ]", 1029., 185., 78., 22.),
                ],
            },
            TextLine {
                text: "Velator•".into(),
                words: vec![word("Velator•", 971., 214., 91., 16.)],
            },
        ],
    };
    assert_eq!(
        ocr::identify_card(&text, &cards, &c).unwrap().ship,
        "Velator"
    );
}

#[test]
fn bundled_database_is_read_only_deduplicated_and_separate_from_user_records() {
    let temp = Temp::new();
    let path = temp.0.join("starter # database.sqlite3");
    let c = catalog();
    let first = mail(&c, 1, "200mm AutoCannon II");
    let second = mail(&c, 2, "280mm Howitzer Artillery II");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT); CREATE TABLE killmails(id INTEGER PRIMARY KEY,ship,pilot,happened,raw TEXT); CREATE TABLE days(day TEXT PRIMARY KEY,source_count,index_total); INSERT INTO days VALUES('20261027',2,2);").unwrap();
    let upstream: Value = serde_json::from_str(include_str!("../native/upstream.json")).unwrap();
    for (key, value) in [
        ("schema_version", json!(1)),
        ("sde_build", upstream["sde_build"].clone()),
        ("complete_days", json!(90)),
        ("period_start", json!("2026-07-30")),
        ("period_end", json!("2026-10-27")),
        ("built_at", json!("fixture")),
        ("records", json!(2)),
        ("known_hulls", json!(423)),
        ("hulls_with_records", json!(1)),
    ] {
        db.execute(
            "INSERT INTO meta VALUES(?,?)",
            rusqlite::params![key, value.to_string()],
        )
        .unwrap();
    }
    for m in [&first, &second] {
        db.execute(
            "INSERT INTO killmails VALUES(?,?,?,?,?)",
            rusqlite::params![
                m["killmail_id"].as_i64(),
                587,
                m["victim"]["character_id"].as_i64(),
                m["killmail_time"].as_str(),
                m.to_string()
            ],
        )
        .unwrap();
    }
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    crate::seed::attach(&connection, &path).unwrap();
    assert!(
        connection
            .execute("DELETE FROM bundled.killmails", [])
            .is_err()
    );
    drop(connection);
    drop(db);
    let before = std::fs::read(&path).unwrap();
    let mut store = Store::open_with_seed(&temp.0.join("user"), Some(&path)).unwrap();
    assert!(store.seed_error.is_none(), "{:?}", store.seed_error);
    assert_eq!(store.global_ships().unwrap(), vec![587]);
    assert_eq!(store.archive_count("20261027"), 2);
    store.ingest(std::slice::from_ref(&first)).unwrap(); // personal-only duplicate must not mask seed global data
    assert_eq!(store.global_mails(587).unwrap().len(), 2);
    store.ingest_global(&[first]).unwrap();
    assert_eq!(store.global_mails(587).unwrap().len(), 2);
    assert_eq!(store.pilot_mails(587, 90000001).unwrap().len(), 1);
    store
        .ingest(&[mail(&c, 3, "Light Neutron Blaster II")])
        .unwrap();
    assert_eq!(store.global_mails(587).unwrap().len(), 2);
    assert_eq!(store.pilot_mails(587, 90000003).unwrap().len(), 1);
    drop(store);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn filter_report_and_personal_loss_are_independent_of_solo_filter() {
    let c = catalog();
    let mut fleet = mail(&c, 1, "200mm AutoCannon II");
    fleet["attackers"] = json!([{"character_id":1},{"character_id":2},{"character_id":3}]);
    let settings = Settings::default();
    let popular = fits::population(std::slice::from_ref(&fleet), &c, &settings);
    assert_eq!(popular.report.input, 1);
    assert_eq!(popular.report.reasons["too_many_attackers"], 1);
    assert_eq!(popular.total, 0);
    let personal =
        fits::pilot_selection(std::slice::from_ref(&fleet), 90000001, 587, &c, &settings);
    assert!(personal.variant.unwrap().personal);
    assert_eq!(personal.report.eligible, 1);
    fleet["zkb"]["npc"] = json!(true);
    assert!(
        fits::pilot_selection(&[fleet], 90000001, 587, &c, &settings)
            .variant
            .is_none()
    );
}

#[test]
fn compact_public_mail_preserves_fit_and_attacker_filtering() {
    let c = catalog();
    let mut original = mail(&c, 1, "200mm AutoCannon II");
    original["attackers"] = json!(
        (1..=20)
            .map(|i| json!({"character_id":i,"damage_done":123}))
            .collect::<Vec<_>>()
    );
    original["victim"]["items"].as_array_mut().unwrap().extend([
        json!({"flag":27,"item_type_id":tid(&c,"Fusion S"),"quantity_destroyed":100}),
        json!({"flag":87,"item_type_id":tid(&c,"Warrior II"),"quantity_destroyed":2}),
        json!({"flag":5,"item_type_id":tid(&c,"Warp Scrambler II"),"quantity_destroyed":100}),
    ]);
    let compact = crate::sources::compact_mail(&original, &c);
    assert_eq!(compact["attackers"].as_array().unwrap().len(), 6);
    assert_eq!(
        fits::normalize(&original, &c),
        fits::normalize(&compact, &c)
    );
    for max_attackers in 1..=5 {
        let settings = Settings {
            max_attackers,
            ..Default::default()
        };
        assert_eq!(
            fits::population(std::slice::from_ref(&original), &c, &settings).total,
            fits::population(std::slice::from_ref(&compact), &c, &settings).total
        );
    }
    assert!(!compact.to_string().contains("damage_done"));
}

#[test]
fn legacy_statistics_settings_migrate_to_incremental_all_hull_updates() {
    let temp = Temp::new();
    let mut raw = serde_json::to_value(Settings::default()).unwrap();
    raw.as_object_mut().unwrap().remove("statistics_version");
    raw["sync_scope"] = json!(0);
    raw["force_refresh"] = json!(true);
    raw["pilot"] = json!("Preserved pilot");
    let bytes = serde_json::to_vec(&raw).unwrap();
    std::fs::write(temp.0.join("settings.json"), &bytes).unwrap();
    let settings = Settings::load(&temp.0);
    assert_eq!(settings.sync_scope, 2);
    assert!(!settings.force_refresh);
    assert_eq!(settings.pilot, "Preserved pilot");
    assert_eq!(std::fs::read(temp.0.join("settings.json")).unwrap(), bytes);
}

#[test]
fn clipped_ship_names_need_a_unique_catalog_match_in_the_selected_card() {
    use crate::ocr::{Document, TextLine, Word};
    let c = catalog();
    let row = |text: &str, x, y, width| TextLine {
        text: text.into(),
        words: vec![Word {
            text: text.into(),
            x,
            y,
            width,
            height: 10.,
        }],
    };
    let cards = |hull: &str| Document {
        lines: vec![
            row("Vesna83 [CR]", 10., 0., 90.),
            row(hull, 5., 20., 100.),
            row("23 KM", 30., 50., 40.),
            row("Gnosis", 35., 70., 40.),
            row("Machariel*", 160., 20., 80.),
        ],
    };
    for (read, expected) in [
        ("Hurricane Fleet", "Hurricane Fleet Issue"),
        ("Hurricane Flee…", "Hurricane Fleet Issue"),
        ("Hurricane Fleet Issue", "Hurricane Fleet Issue"),
        ("Gnosis", "Gnosis"),
        ("Praxi", "Praxis"),
        ("praxi;", "Praxis"),
        ("Drake Navy ISSI", "Drake Navy Issue"),
        ("Orake Navu", "Drake Navy Issue"),
        ("Apocaiypse Nav", "Apocalypse Navy Issue"),
    ] {
        assert_eq!(
            ocr::identify_card("Vesna83\n23 km", &cards(read), &c)
                .unwrap()
                .ship,
            expected
        );
    }
    // A prefix shared by the base and faction ship is unresolved without a marker.
    assert!(ocr::identify_card("Vesna83", &cards("Hurricane"), &c).is_err());
    // A damaged base name must not choose between base and navy/fleet hulls.
    assert!(ocr::identify_card("Vesna83", &cards("Hurricame"), &c).is_err());
    assert!(ocr::identify_card("Vesna83", &cards("Apocaiypse"), &c).is_err());
    assert!(ocr::identify_card("Vesna83", &cards("Glid"), &c).is_err());
    assert!(ocr::identify_card("Vesna83", &cards("Unknown hull"), &c).is_err());
    let wrapped = Document {
        lines: vec![
            row("Vesna83 [CR]", 10., 0., 90.),
            row("Hurricane", 15., 20., 80.),
            row("Fleet Issue", 15., 32., 80.),
            row("23 KM", 30., 50., 40.),
        ],
    };
    assert_eq!(
        ocr::identify_card("Vesna83", &wrapped, &c).unwrap().ship,
        "Hurricane Fleet Issue"
    );
}

#[test]
fn missing_targets_expire_after_configured_delay_and_changes_clear_immediately() {
    use std::time::{Duration, Instant};
    let c = catalog();
    let a = ocr::parse("Pilot: First Pilot\nRifter", &c).unwrap();
    let b = ocr::parse("Pilot: Other Pilot\nTristan", &c).unwrap();
    let start = Instant::now();
    let mut gate = Gate::default();
    assert!(gate.feed_at(Some(a.clone()), start, 60).is_none());
    assert_eq!(
        gate.feed_at(Some(a.clone()), start, 60),
        Some(Some(a.clone()))
    );
    assert!(gate.feed_at(None, start, 60).is_none());
    assert!(
        gate.feed_at(None, start + Duration::from_secs(59), 60)
            .is_none()
    );
    assert_eq!(
        gate.remaining(start + Duration::from_secs(59), 60),
        Some(Duration::from_secs(1))
    );
    assert!(
        gate.feed_at(Some(a.clone()), start + Duration::from_secs(59), 60)
            .is_none()
    );
    assert_eq!(gate.remaining(start + Duration::from_secs(60), 60), None);
    assert!(
        gate.feed_at(None, start + Duration::from_secs(61), 60)
            .is_none()
    );
    assert!(!gate.expire(start + Duration::from_secs(120), 60));
    assert!(gate.expire(start + Duration::from_secs(121), 60));
    assert!(!gate.expire(start + Duration::from_secs(122), 60));
    gate.feed_at(Some(a.clone()), start, 60);
    gate.feed_at(Some(a.clone()), start, 60);
    gate.feed_at(None, start, 60);
    assert_eq!(
        gate.feed_at(Some(b.clone()), start + Duration::from_secs(2), 60),
        Some(None)
    );
    assert_eq!(gate.remaining(start + Duration::from_secs(2), 60), None);
    assert_eq!(
        gate.feed_at(Some(b.clone()), start + Duration::from_secs(3), 60),
        Some(Some(b))
    );
    assert_eq!(
        gate.feed_at(None, start + Duration::from_secs(4), 0),
        Some(None)
    );
    assert!(ocr::clearly_different_pilot("Other Pilot", "First Pilot"));
    assert!(!ocr::clearly_different_pilot("Kennedy 518", "Kennedu 518"));
    let temp = Temp::new();
    assert_eq!(Settings::load(&temp.0).target_hold_seconds, 60);
}

#[test]
fn base_resistance_holes_break_ties_without_overriding_real_resistance_differences() {
    let total = [50.; 4];
    let base = [0., 20., 40., 50.];
    let em = tactics::base_hole_preference(&[1., 0., 0., 0.], &total, &base);
    let explosive = tactics::base_hole_preference(&[0., 0., 0., 1.], &total, &base);
    assert!(em > explosive);
    assert_eq!(
        tactics::base_hole_preference(&[1., 0., 0., 0.], &[0., 20., 40., 50.], &base),
        0.
    );
    assert_eq!(
        tactics::base_hole_preference(&[1., 0., 0., 0.], &total, &[40.; 4]),
        tactics::base_hole_preference(&[0., 0., 0., 1.], &total, &[40.; 4])
    );
}

#[test]
fn space_labels_do_not_override_a_valid_target_card_and_small_unique_prefixes_recover() {
    let c = catalog();
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(
        "../resources/fixtures/ocr-background-cards.json"
    ))
    .unwrap();
    for fixture in fixtures.as_array().unwrap() {
        let cards: ocr::Document = serde_json::from_value(fixture["cards"].clone()).unwrap();
        let target =
            ocr::identify_with_retries(fixture["selected"].as_str().unwrap(), &[], &cards, &c)
                .unwrap();
        assert_eq!(target.ship, fixture["ship"].as_str().unwrap());
    }
}
