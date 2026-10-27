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
