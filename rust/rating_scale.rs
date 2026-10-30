//! Shared seven-color palette and data-derived tier boundaries.
use eframe::egui::Color32;
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Deserialize)]
struct Color {
    rgb: [u8; 3],
}
#[derive(Deserialize)]
pub struct Scale {
    pub boundaries: [u16; 6],
}
#[derive(Deserialize)]
pub struct WinrateScale {
    pub boundaries: [u8; 6],
    pub minimum_battles: u64,
}
#[derive(Deserialize)]
pub struct Reference {
    colors: [Color; 7],
    pub solo: Scale,
    pub winrate: WinrateScale,
}
pub fn reference() -> &'static Reference {
    static REFERENCE: OnceLock<Reference> = OnceLock::new();
    REFERENCE.get_or_init(|| {
        serde_json::from_str(include_str!("../resources/rating-tiers.json"))
            .expect("bundled seven-tier calibration")
    })
}
pub fn color(tier: usize) -> Color32 {
    let [r, g, b] = reference().colors[tier.min(6)].rgb;
    Color32::from_rgb(r, g, b)
}
pub fn solo_tier(score: u16) -> usize {
    reference()
        .solo
        .boundaries
        .partition_point(|cut| score >= *cut)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_palette_has_seven_distinct_levels_with_green_at_fifty_percent() {
        let r = reference();
        assert!(r.solo.boundaries.windows(2).all(|p| p[0] < p[1]));
        assert!(r.winrate.boundaries.windows(2).all(|p| p[0] < p[1]));
        assert_eq!(r.winrate.boundaries.partition_point(|v| 50 >= *v), 3);
        assert_eq!(color(3), Color32::from_rgb(116, 205, 118));
        assert_eq!(color(5), Color32::from_rgb(95, 153, 241));
        assert_eq!(solo_tier(1000), 0);
        assert_eq!(solo_tier(8999), 6);
    }
}
