//! `db seed destination-refs` — port of scripts/seed-destination-refs.ts.
//!
//! Seeds normalized destination reference data into Turso. Data is inlined as
//! typed Rust constants (curated reference content, not fetched), so no JSON
//! file is read at runtime. Idempotent via INSERT OR REPLACE.
//!
//! Tables: destination_areas, destination_area_stations, destination_area_best_for,
//! destination_pois, destination_poi_tags, destination_clusters,
//! destination_cluster_pois, destination_transit, destination_tips.
//!
//! Signature fixed by main.rs dispatch: `run(&[String])`.

use crate::db::connect_write;
use libsql::params;

const SOURCE_URL: &str = "repo:src/skills/travel-shared/references/destinations/*.json#v1.0.0";
const CONFIDENCE: &str = "curated";

// ── data types ───────────────────────────────────────────────────────────────

struct Area {
    id: &'static str,
    name: &'static str,
    type_: &'static str,
    stations: &'static [&'static str],
    vibe: &'static str,
    best_for: &'static [&'static str],
}

struct Poi {
    id: &'static str,
    title: &'static str,
    area: &'static str,
    nearest_station: &'static str,
    duration_min: i64,
    booking_required: bool,
    booking_url: Option<&'static str>,
    cost_estimate: i64,
    tags: &'static [&'static str],
    notes: Option<&'static str>,
    hours: Option<&'static str>,
    address: Option<&'static str>,
    lat: Option<f64>,
    lon: Option<f64>,
}

struct Cluster {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    pois: &'static [&'static str],
    duration_min: i64,
    best_area: &'static str,
}

struct Transit {
    pair_key: &'static str,
    kind: &'static str,
    minutes: i64,
    line: &'static str,
    station_from: Option<&'static str>,
    station_to: Option<&'static str>,
}

struct DestRef {
    slug: &'static str,
    areas: &'static [Area],
    pois: &'static [Poi],
    clusters: &'static [Cluster],
    transit: &'static [Transit],
    tips: &'static [&'static str],
}

// ── data ─────────────────────────────────────────────────────────────────────

fn data() -> Vec<DestRef> {
    vec![
        // ── tokyo_2026 ───────────────────────────────────────────────────
        DestRef {
            slug: "tokyo_2026",
            areas: &[
                Area { id: "shinjuku", name: "Shinjuku", type_: "commercial",
                    stations: &["Shinjuku", "Shinjuku-sanchome", "Nishi-Shinjuku"],
                    vibe: "Busy commercial hub, nightlife, shopping",
                    best_for: &["shopping", "nightlife", "food"] },
                Area { id: "shibuya", name: "Shibuya", type_: "commercial",
                    stations: &["Shibuya", "Harajuku", "Omotesando"],
                    vibe: "Youth culture, fashion, iconic crossing",
                    best_for: &["shopping", "culture", "food"] },
                Area { id: "asakusa", name: "Asakusa", type_: "traditional",
                    stations: &["Asakusa", "Tawaramachi"],
                    vibe: "Traditional Tokyo, temples, old-town charm",
                    best_for: &["temple", "traditional", "souvenirs"] },
                Area { id: "roppongi", name: "Roppongi", type_: "entertainment",
                    stations: &["Roppongi", "Azabu-juban", "Kamiyacho"],
                    vibe: "Upscale dining, art museums, nightlife",
                    best_for: &["art", "dining", "nightlife"] },
                Area { id: "ginza", name: "Ginza", type_: "luxury",
                    stations: &["Ginza", "Yurakucho", "Higashi-Ginza"],
                    vibe: "High-end shopping, department stores",
                    best_for: &["luxury_shopping", "dining", "theatre"] },
                Area { id: "akihabara", name: "Akihabara", type_: "specialty",
                    stations: &["Akihabara", "Suehirocho"],
                    vibe: "Electronics, anime, otaku culture",
                    best_for: &["electronics", "anime", "gaming"] },
                Area { id: "hamamatsucho", name: "Hamamatsucho", type_: "business",
                    stations: &["Hamamatsucho", "Daimon", "Takeshiba"],
                    vibe: "Business district, transit hub, Hamarikyu Gardens",
                    best_for: &["transit", "gardens", "quiet"] },
                Area { id: "tokyo_station", name: "Tokyo Station", type_: "transit",
                    stations: &["Tokyo", "Marunouchi", "Otemachi"],
                    vibe: "Major transit hub, excellent omiyage shopping",
                    best_for: &["transit", "omiyage", "food"] },
                Area { id: "azabudai", name: "Azabudai Hills", type_: "modern",
                    stations: &["Kamiyacho", "Roppongi-itchome"],
                    vibe: "New development, teamLab Borderless, upscale",
                    best_for: &["art", "dining", "modern"] },
            ],
            pois: &[
                Poi { id: "komehyo_shinjuku", title: "KOMEHYO Shinjuku", area: "shinjuku",
                    nearest_station: "Shinjuku", duration_min: 90, booking_required: false,
                    booking_url: None, cost_estimate: 0,
                    tags: &["shopping", "luxury", "second_hand", "bags"],
                    notes: Some("5 floors, largest second-hand luxury in Tokyo. Best for Chanel, LV, Hermes."),
                    hours: Some("11:00-20:00"), address: Some("3-26-6 Shinjuku, Shinjuku-ku"), lat: None, lon: None },
                Poi { id: "daikokuya_shinjuku", title: "Daikokuya Shinjuku", area: "shinjuku",
                    nearest_station: "Shinjuku", duration_min: 45, booking_required: false,
                    booking_url: None, cost_estimate: 0,
                    tags: &["shopping", "luxury", "second_hand", "bags"],
                    notes: Some("Good backup for Chanel. 5 min walk from KOMEHYO."),
                    hours: Some("10:30-19:30"), address: None, lat: None, lon: None },
                Poi { id: "isetan_shinjuku", title: "Isetan Shinjuku B2", area: "shinjuku",
                    nearest_station: "Shinjuku-sanchome", duration_min: 60, booking_required: false,
                    booking_url: None, cost_estimate: 5000,
                    tags: &["shopping", "omiyage", "department_store", "food"],
                    notes: Some("Premium omiyage: Yoku Moku, Henri Charpentier, Pierre Herme. B1-B2 food halls."),
                    hours: Some("10:00-20:00"), address: None, lat: None, lon: None },
                Poi { id: "omoide_yokocho", title: "Omoide Yokocho (Memory Lane)", area: "shinjuku",
                    nearest_station: "Shinjuku", duration_min: 60, booking_required: false,
                    booking_url: None, cost_estimate: 2000,
                    tags: &["food", "izakaya", "yakitori", "nightlife"],
                    notes: Some("Narrow alley of tiny yakitori bars. Atmospheric. Cash only."),
                    hours: Some("17:00-24:00"), address: None, lat: None, lon: None },
                Poi { id: "teamlab_borderless", title: "teamLab Borderless", area: "azabudai",
                    nearest_station: "Kamiyacho", duration_min: 150, booking_required: true,
                    booking_url: Some("https://www.teamlab.art/e/borderless-azabudai/"), cost_estimate: 3800,
                    tags: &["art", "museum", "digital", "experience"],
                    notes: Some("New location at Azabudai Hills (opened Feb 2024). Book in advance!"),
                    hours: Some("10:00-21:00"), address: None, lat: None, lon: None },
                Poi { id: "sensoji", title: "Senso-ji Temple", area: "asakusa",
                    nearest_station: "Asakusa", duration_min: 60, booking_required: false,
                    booking_url: None, cost_estimate: 0,
                    tags: &["temple", "traditional", "sightseeing", "free"],
                    notes: Some("Tokyo's oldest temple. Kaminarimon Gate, Nakamise shopping street."),
                    hours: Some("06:00-17:00 (temple), shops 10:00-17:00"), address: None, lat: None, lon: None },
                Poi { id: "nakamise", title: "Nakamise Shopping Street", area: "asakusa",
                    nearest_station: "Asakusa", duration_min: 30, booking_required: false,
                    booking_url: None, cost_estimate: 1000,
                    tags: &["shopping", "traditional", "souvenirs", "snacks"],
                    notes: Some("Traditional souvenirs, snacks (ningyo-yaki, senbei). Connected to Senso-ji."),
                    hours: None, address: None, lat: None, lon: None },
                Poi { id: "shibuya_crossing", title: "Shibuya Crossing", area: "shibuya",
                    nearest_station: "Shibuya", duration_min: 15, booking_required: false,
                    booking_url: None, cost_estimate: 0,
                    tags: &["sightseeing", "iconic", "free"],
                    notes: Some("World's busiest pedestrian crossing. Best viewed from Starbucks or Shibuya Sky."),
                    hours: None, address: None, lat: None, lon: None },
                Poi { id: "shibuya_sky", title: "Shibuya Sky", area: "shibuya",
                    nearest_station: "Shibuya", duration_min: 60, booking_required: true,
                    booking_url: Some("https://www.shibuya-scramble-square.com/sky/"), cost_estimate: 2200,
                    tags: &["observation", "sightseeing", "photo"],
                    notes: Some("360° rooftop observation deck. Best at sunset. Book in advance."),
                    hours: None, address: None, lat: None, lon: None },
                Poi { id: "takeshita_street", title: "Takeshita Street", area: "shibuya",
                    nearest_station: "Harajuku", duration_min: 45, booking_required: false,
                    booking_url: None, cost_estimate: 1000,
                    tags: &["shopping", "youth", "fashion", "crepes"],
                    notes: Some("Youth fashion, quirky shops, crepes. Very crowded on weekends."),
                    hours: None, address: None, lat: None, lon: None },
                Poi { id: "daimaru_tokyo", title: "Daimaru Tokyo Station B1", area: "tokyo_station",
                    nearest_station: "Tokyo", duration_min: 45, booking_required: false,
                    booking_url: None, cost_estimate: 5000,
                    tags: &["shopping", "omiyage", "department_store", "food"],
                    notes: Some("Best for last-minute omiyage. Tokyo Banana, all classic gifts. Right in station."),
                    hours: Some("10:00-20:00"), address: None, lat: None, lon: None },
                Poi { id: "tokyo_ramen_street", title: "Tokyo Ramen Street", area: "tokyo_station",
                    nearest_station: "Tokyo", duration_min: 45, booking_required: false,
                    booking_url: None, cost_estimate: 1200,
                    tags: &["food", "ramen", "quick"],
                    notes: Some("8 famous ramen shops in Tokyo Station B1. Popular = expect lines."),
                    hours: None, address: None, lat: None, lon: None },
                Poi { id: "hamarikyu_gardens", title: "Hamarikyu Gardens", area: "hamamatsucho",
                    nearest_station: "Shiodome", duration_min: 60, booking_required: false,
                    booking_url: None, cost_estimate: 300,
                    tags: &["garden", "nature", "quiet", "tea"],
                    notes: Some("Beautiful traditional garden near Hamamatsucho. Matcha tea house on island."),
                    hours: None, address: None, lat: None, lon: None },
                Poi { id: "roppongi_hills", title: "Roppongi Hills", area: "roppongi",
                    nearest_station: "Roppongi", duration_min: 90, booking_required: false,
                    booking_url: None, cost_estimate: 0,
                    tags: &["shopping", "art", "observation", "dining"],
                    notes: Some("Mori Art Museum, Tokyo City View, upscale shopping and dining."),
                    hours: None, address: None, lat: None, lon: None },
            ],
            clusters: &[
                Cluster { id: "chanel_shopping", name: "Second-Hand Luxury Shopping",
                    description: "Best stores for pre-owned Chanel bags",
                    pois: &["komehyo_shinjuku", "daikokuya_shinjuku"], duration_min: 150, best_area: "shinjuku" },
                Cluster { id: "omiyage_premium", name: "Premium Omiyage",
                    description: "High-quality gift shopping at department stores",
                    pois: &["isetan_shinjuku", "daimaru_tokyo"], duration_min: 90, best_area: "shinjuku" },
                Cluster { id: "asakusa_classic", name: "Classic Asakusa",
                    description: "Traditional Tokyo experience",
                    pois: &["sensoji", "nakamise"], duration_min: 120, best_area: "asakusa" },
                Cluster { id: "shibuya_harajuku", name: "Shibuya + Harajuku",
                    description: "Youth culture, fashion, iconic spots",
                    pois: &["shibuya_crossing", "takeshita_street", "shibuya_sky"], duration_min: 180, best_area: "shibuya" },
                Cluster { id: "teamlab_roppongi", name: "teamLab + Roppongi",
                    description: "Digital art and upscale district",
                    pois: &["teamlab_borderless", "roppongi_hills"], duration_min: 240, best_area: "azabudai" },
                Cluster { id: "last_day_tokyo_station", name: "Last-Day Tokyo Station",
                    description: "Final omiyage run before departure",
                    pois: &["daimaru_tokyo", "tokyo_ramen_street"], duration_min: 90, best_area: "tokyo_station" },
            ],
            transit: &[
                Transit { pair_key: "hamamatsucho_to_shinjuku", kind: "estimate", minutes: 25,
                    line: "JR Yamanote", station_from: None, station_to: None },
                Transit { pair_key: "hamamatsucho_to_shibuya", kind: "estimate", minutes: 20,
                    line: "JR Yamanote", station_from: None, station_to: None },
                Transit { pair_key: "hamamatsucho_to_asakusa", kind: "estimate", minutes: 20,
                    line: "Toei Asakusa Line", station_from: None, station_to: None },
                Transit { pair_key: "hamamatsucho_to_roppongi", kind: "estimate", minutes: 15,
                    line: "Toei Oedo Line", station_from: None, station_to: None },
                Transit { pair_key: "hamamatsucho_to_kamiyacho", kind: "estimate", minutes: 10,
                    line: "Hibiya Line", station_from: None, station_to: None },
                Transit { pair_key: "hamamatsucho_to_tokyo", kind: "estimate", minutes: 5,
                    line: "JR Yamanote", station_from: None, station_to: None },
                Transit { pair_key: "shinjuku_to_shibuya", kind: "estimate", minutes: 10,
                    line: "JR Yamanote", station_from: None, station_to: None },
                Transit { pair_key: "shibuya_to_harajuku", kind: "estimate", minutes: 3,
                    line: "JR Yamanote or walk", station_from: None, station_to: None },
                Transit { pair_key: "asakusa_to_shibuya", kind: "estimate", minutes: 30,
                    line: "Ginza Line", station_from: None, station_to: None },
                Transit { pair_key: "tokyo_to_narita", kind: "estimate", minutes: 60,
                    line: "Narita Express", station_from: None, station_to: None },
            ],
            tips: &[
                "Get Suica/Pasmo IC card at airport for all transit",
                "KOMEHYO opens at 11:00 - don't arrive early",
                "teamLab Borderless sells out - book 1-2 weeks ahead",
                "Omoide Yokocho is cash only",
                "Tokyo Station omiyage shops open until 20:00 or later",
            ],
        },
        // ── osaka_2026 ───────────────────────────────────────────────────
        DestRef {
            slug: "osaka_2026",
            areas: &[
                Area { id: "namba", name: "Namba", type_: "commercial",
                    stations: &["Namba", "Shinsaibashi"],
                    vibe: "Street food, nightlife, Dotonbori",
                    best_for: &["food", "nightlife", "shopping"] },
                Area { id: "umeda", name: "Umeda / Osaka Station", type_: "commercial",
                    stations: &["Osaka", "Umeda", "Nishi-Umeda"],
                    vibe: "Major transit hub, department stores, observation decks",
                    best_for: &["transit", "shopping", "views"] },
                Area { id: "shinsekai", name: "Shinsekai", type_: "traditional",
                    stations: &["Dobutsuen-mae", "Shin-Imamiya"],
                    vibe: "Retro district, kushikatsu, Tsutenkaku Tower",
                    best_for: &["food", "culture", "sightseeing"] },
            ],
            pois: &[],
            clusters: &[],
            transit: &[],
            tips: &[],
        },
        // ── osaka_kyoto_2026 ─────────────────────────────────────────────
        DestRef {
            slug: "osaka_kyoto_2026",
            areas: &[
                Area { id: "namba", name: "Namba", type_: "commercial",
                    stations: &["Namba", "Osaka Namba", "Nippombashi"],
                    vibe: "Vibrant nightlife, shopping, food paradise",
                    best_for: &["food", "nightlife", "shopping", "entertainment"] },
                Area { id: "shinsaibashi", name: "Shinsaibashi", type_: "commercial",
                    stations: &["Shinsaibashi", "Osaka Namba"],
                    vibe: "Upscale shopping arcades, boutique stores",
                    best_for: &["shopping", "fashion", "dining"] },
                Area { id: "dotonbori", name: "Dotonbori", type_: "food",
                    stations: &["Namba", "Osaka Namba"],
                    vibe: "Iconic food street, neon signs, street food heaven",
                    best_for: &["food", "nightlife", "sightseeing", "photo"] },
                Area { id: "umeda", name: "Umeda", type_: "commercial",
                    stations: &["Umeda", "Osaka", "Higashi-Umeda"],
                    vibe: "Major transit hub, department stores, modern",
                    best_for: &["shopping", "transit", "dining", "architecture"] },
                Area { id: "osaka_castle", name: "Osaka Castle Area", type_: "historical",
                    stations: &["Osaka-johi", "Morinomiya", "Tanimachi-yonchome"],
                    vibe: "Historic castle, cherry blossoms, samurai history",
                    best_for: &["history", "sightseeing", "nature", "photo"] },
                Area { id: "shinsekai", name: "Shinsekai", type_: "traditional",
                    stations: &["Shinsekai", "Spa World"],
                    vibe: "Retro Osaka, kushiage, traditional atmosphere",
                    best_for: &["food", "traditional", "photo"] },
                Area { id: "kyoto_station", name: "Kyoto Station", type_: "transit",
                    stations: &["Kyoto"],
                    vibe: "Modern architecture, transit hub, shopping complex",
                    best_for: &["transit", "shopping", "dining"] },
                Area { id: "higashiyama", name: "Higashiyama", type_: "traditional",
                    stations: &["Gion-Shijo", "Kawaramachi", "Higashiyama"],
                    vibe: "Traditional Kyoto, geisha district, old town",
                    best_for: &["traditional", "culture", "temples", "walking"] },
                Area { id: "arashiyama", name: "Arashiyama", type_: "nature",
                    stations: &["Arashiyama", "Saga-Arashiyama"],
                    vibe: "Bamboo grove, scenic mountains, temples",
                    best_for: &["nature", "temples", "hiking", "photo"] },
                Area { id: "fushimi", name: "Fushimi Inari", type_: "temple",
                    stations: &["Inari"],
                    vibe: "Famous torii gates, spiritual, hiking",
                    best_for: &["temples", "hiking", "iconic", "photography"] },
                Area { id: "kinkakuji", name: "Kinkaku-ji", type_: "temple",
                    stations: &["Kinkaji-mae", "Kita-Oji"],
                    vibe: "Golden pavilion, zen garden, must-see",
                    best_for: &["temples", "gardens", "iconic", "sightseeing"] },
                Area { id: "gion", name: "Gion", type_: "traditional",
                    stations: &["Gion-Shijo"],
                    vibe: "Geisha district, historic machiya, tea houses",
                    best_for: &["culture", "traditional", "geisha", "walking"] },
            ],
            pois: &[
                Poi { id: "dotonbori", title: "Dotonbori Canal", area: "dotonbori",
                    nearest_station: "Namba", duration_min: 90, booking_required: false,
                    booking_url: None, cost_estimate: 2000,
                    tags: &["food", "nightlife", "sightseeing", "photo"],
                    notes: Some("Iconic Glico running man sign. Try takoyaki, okonomiyaki, kushikatsu. Best at night."),
                    hours: Some("24/7 (shops vary)"), address: None, lat: None, lon: None },
                Poi { id: "kuromon_market", title: "Kuromon Ichiba Market", area: "namba",
                    nearest_station: "Nippombashi", duration_min: 60, booking_required: false,
                    booking_url: None, cost_estimate: 3000,
                    tags: &["food", "market", "seafood", "omiyage"],
                    notes: Some("Fresh seafood, crab, strawberries, dried goods. Similar to Tokyo's Tsukiji."),
                    hours: Some("09:00-18:00"), address: None, lat: None, lon: None },
                Poi { id: "shinsaibashi_shopping", title: "Shinsaibashi Shopping Arcade", area: "shinsaibashi",
                    nearest_station: "Shinsaibashi", duration_min: 90, booking_required: false,
                    booking_url: None, cost_estimate: 0,
                    tags: &["shopping", "arcade", "fashion"],
                    notes: Some("600m covered arcade. American Village nearby for youth fashion."),
                    hours: Some("10:00-20:00"), address: None, lat: None, lon: None },
                Poi { id: "osaka_castle", title: "Osaka Castle", area: "osaka_castle",
                    nearest_station: "Osaka-johi", duration_min: 120, booking_required: false,
                    booking_url: None, cost_estimate: 600,
                    tags: &["history", "castle", "museum", "garden"],
                    notes: Some("Original fortress built 1597. Excellent views from top floor. Cherry blossoms in spring."),
                    hours: Some("09:00-17:00"), address: None, lat: None, lon: None },
                Poi { id: "tenjinbashi", title: "Tenjinbashisuji Shopping Street", area: "umeda",
                    nearest_station: "Umeda", duration_min: 45, booking_required: false,
                    booking_url: None, cost_estimate: 1000,
                    tags: &["shopping", "traditional", "local"],
                    notes: Some("Longest shopping street in Japan (2.6km). Local shops, some foreign goods."),
                    hours: Some("Shops 10:00-18:00"), address: None, lat: None, lon: None },
                Poi { id: "fushimi_inari", title: "Fushimi Inari Taisha", area: "fushimi",
                    nearest_station: "Inari", duration_min: 180, booking_required: false,
                    booking_url: None, cost_estimate: 0,
                    tags: &["temple", "hiking", "iconic", "free"],
                    notes: Some("10,000+ torii gates. Hike to top takes 2-3 hours. Go early (6am) to avoid crowds."),
                    hours: Some("24/7"), address: None, lat: None, lon: None },
                Poi { id: "bamboo_grove", title: "Arashiyama Bamboo Grove", area: "arashiyama",
                    nearest_station: "Arashiyama", duration_min: 60, booking_required: false,
                    booking_url: None, cost_estimate: 0,
                    tags: &["nature", "iconic", "walking", "photography"],
                    notes: Some("Arashiyama Bamboo Forest. Best at dawn or dusk. Combine with Tenryu-ji temple."),
                    hours: Some("24/7 (free), Tenryu-ji 08:00-17:00"), address: None, lat: None, lon: None },
                Poi { id: "tenryuji", title: "Tenryu-ji Temple", area: "arashiyama",
                    nearest_station: "Arashiyama", duration_min: 60, booking_required: false,
                    booking_url: None, cost_estimate: 500,
                    tags: &["temple", "garden", "zen", "unesco"],
                    notes: Some("UNESCO World Heritage. Stunning Zen garden. Less crowded than bamboo grove."),
                    hours: Some("08:00-17:00"), address: None, lat: None, lon: None },
                Poi { id: "kinkakuji", title: "Kinkaku-ji (Golden Pavilion)", area: "kinkakuji",
                    nearest_station: "Kinkaji-mae", duration_min: 45, booking_required: false,
                    booking_url: None, cost_estimate: 500,
                    tags: &["temple", "iconic", "gardens", "unesco"],
                    notes: Some("Gold-leaf covered pavilion reflected in pond. Very popular, go early."),
                    hours: Some("09:00-17:00"), address: None, lat: None, lon: None },
                Poi { id: "kiyomizudera", title: "Kiyomizu-dera Temple", area: "higashiyama",
                    nearest_station: "Kiyomizu-Gojo", duration_min: 90, booking_required: false,
                    booking_url: None, cost_estimate: 500,
                    tags: &["temple", "iconic", "views", "unesco"],
                    notes: Some("Wooden stage with panoramic Kyoto views. UNESCO site. Jishu shrine nearby for love luck."),
                    hours: Some("06:00-18:00"), address: None, lat: None, lon: None },
                Poi { id: "gion_walk", title: "Gion District Walk", area: "gion",
                    nearest_station: "Gion-Shijo", duration_min: 90, booking_required: false,
                    booking_url: None, cost_estimate: 0,
                    tags: &["walking", "traditional", "culture", "geisha"],
                    notes: Some("Historic merchant district. Look for maiko (apprentice geisha) in evening. Yasaka Pagoda nearby."),
                    hours: Some("Any time, evening for atmosphere"), address: None, lat: None, lon: None },
                Poi { id: "nishiki_market", title: "Nishiki Market", area: "kyoto_station",
                    nearest_station: "Shijo", duration_min: 60, booking_required: false,
                    booking_url: None, cost_estimate: 2000,
                    tags: &["food", "market", "traditional", "ingredients"],
                    notes: Some("Kitchen of Kyoto. 400+ years of food stalls. Try yuba (tofu skin), tsukemono, sweets."),
                    hours: Some("09:00-17:00"), address: None, lat: None, lon: None },
                Poi { id: "philosopher_path", title: "Philosopher's Path", area: "higashiyama",
                    nearest_station: "Kita-Oji", duration_min: 60, booking_required: false,
                    booking_url: None, cost_estimate: 0,
                    tags: &["walking", "nature", "cherry_blossoms", "quiet"],
                    notes: Some("2km stone path along canal. Cherry blossoms in early April. Connects to Ginkaku-ji."),
                    hours: Some("24/7"), address: None, lat: None, lon: None },
                Poi { id: "universal_studios", title: "Universal Studios Japan", area: "namba",
                    nearest_station: "Universal City", duration_min: 300, booking_required: true,
                    booking_url: Some("https://www.usj.co.jp/"), cost_estimate: 9400,
                    tags: &["theme_park", "rides", "entertainment", "Nintendo"],
                    notes: Some("Super Nintendo World, Harry Potter, Jurassic Park. Express Pass recommended for busy days."),
                    hours: Some("09:00-21:00 (varies by season)"), address: None, lat: None, lon: None },
            ],
            clusters: &[
                Cluster { id: "osaka_food_hell", name: "Osaka Food Heaven",
                    description: "Best of Osaka's street food scene",
                    pois: &["dotonbori", "kuromon_market", "shinsaibashi_shopping"], duration_min: 240, best_area: "namba" },
                Cluster { id: "kyoto_temple_run", name: "Essential Kyoto Temples",
                    description: "Classic Kyoto temple visits",
                    pois: &["kinkakuji", "kiyomizudera", "fushimi_inari"], duration_min: 360, best_area: "higashiyama" },
                Cluster { id: "arashiyama_day", name: "Arashiyama Nature Day",
                    description: "Bamboo, temples, and mountains",
                    pois: &["bamboo_grove", "tenryuji", "philosopher_path"], duration_min: 240, best_area: "arashiyama" },
                Cluster { id: "kyoto_traditional", name: "Traditional Kyoto Walk",
                    description: "Gion, Higashiyama old town",
                    pois: &["gion_walk", "kiyomizudera", "nishiki_market"], duration_min: 300, best_area: "gion" },
                Cluster { id: "osaka_history", name: "Osaka Castle & History",
                    description: "Samurai history and castle grounds",
                    pois: &["osaka_castle", "tenjinbashi"], duration_min: 180, best_area: "osaka_castle" },
            ],
            transit: &[
                Transit { pair_key: "kix_to_namba", kind: "estimate", minutes: 45,
                    line: "Nankai Airport Express", station_from: None, station_to: None },
                Transit { pair_key: "kix_to_kyoto", kind: "estimate", minutes: 90,
                    line: "JR Haruka", station_from: None, station_to: None },
                Transit { pair_key: "namba_to_kyoto", kind: "estimate", minutes: 45,
                    line: "JR Kyoto Line", station_from: None, station_to: None },
                Transit { pair_key: "namba_to_osaka_castle", kind: "estimate", minutes: 15,
                    line: "Osaka Metro", station_from: None, station_to: None },
                Transit { pair_key: "kyoto_station_to_gion", kind: "estimate", minutes: 15,
                    line: "Keihan Line", station_from: None, station_to: None },
                Transit { pair_key: "kyoto_station_to_arashiyama", kind: "estimate", minutes: 15,
                    line: "JR Sagano Line", station_from: None, station_to: None },
                Transit { pair_key: "kyoto_station_to_fushimi_inari", kind: "estimate", minutes: 5,
                    line: "JR Nara Line", station_from: None, station_to: None },
                Transit { pair_key: "umeda_to_namba", kind: "estimate", minutes: 10,
                    line: "Osaka Metro", station_from: None, station_to: None },
                Transit { pair_key: "osaka_to_kyoto", kind: "inter_city", minutes: 15,
                    line: "JR Tokaido Shinkansen", station_from: Some("Osaka"), station_to: Some("Kyoto") },
                Transit { pair_key: "kyoto_to_osaka", kind: "inter_city", minutes: 15,
                    line: "JR Tokaido Shinkansen", station_from: Some("Kyoto"), station_to: Some("Osaka") },
            ],
            tips: &[
                "Get ICOCA card at KIX for all JR/bus/subway travel",
                "Kyoto bus pass (500 yen/day) covers most attractions",
                "Fushimi Inari opens 24/7 - go at sunrise (6am) to avoid crowds",
                "Osaka food is cheaper than Tokyo - eat generously at Kuromon Market",
                "Day-trip from Osaka to Kyoto is easy (15 min by shinkansen, 45 min by regular JR)",
                "Universal Studios Japan requires advance booking on weekends",
                "Gion most atmospheric at dusk - watch for maiko leaving teahouses",
            ],
        },
        // ── nagoya_2026 ──────────────────────────────────────────────────
        DestRef {
            slug: "nagoya_2026",
            areas: &[
                Area { id: "nagoya_station", name: "Nagoya Station", type_: "commercial",
                    stations: &["Nagoya"],
                    vibe: "Major transit hub, shopping complexes",
                    best_for: &["transit", "shopping", "food"] },
                Area { id: "sakae", name: "Sakae", type_: "commercial",
                    stations: &["Sakae", "Yaba-cho"],
                    vibe: "Downtown entertainment and shopping district",
                    best_for: &["shopping", "nightlife", "food"] },
            ],
            pois: &[],
            clusters: &[],
            transit: &[],
            tips: &[],
        },
        // ── kyoto_2026 ───────────────────────────────────────────────────
        DestRef {
            slug: "kyoto_2026",
            areas: &[
                Area { id: "kyoto_station", name: "Kyoto Station", type_: "transit",
                    stations: &["Kyoto"],
                    vibe: "Modern architecture, transit hub, shopping complex",
                    best_for: &["transit", "shopping", "dining"] },
                Area { id: "higashiyama", name: "Higashiyama", type_: "traditional",
                    stations: &["Gion-Shijo", "Kawaramachi", "Higashiyama"],
                    vibe: "Traditional Kyoto, geisha district, old town",
                    best_for: &["traditional", "culture", "temples", "walking"] },
                Area { id: "arashiyama", name: "Arashiyama", type_: "nature",
                    stations: &["Arashiyama", "Saga-Arashiyama"],
                    vibe: "Bamboo grove, scenic mountains, temples",
                    best_for: &["nature", "temples", "hiking", "photo"] },
                Area { id: "fushimi", name: "Fushimi Inari", type_: "temple",
                    stations: &["Inari"],
                    vibe: "Famous torii gates, spiritual, hiking",
                    best_for: &["temples", "hiking", "iconic", "photography"] },
                Area { id: "kinkakuji", name: "Kinkaku-ji", type_: "temple",
                    stations: &["Kinkaji-mae", "Kita-Oji"],
                    vibe: "Golden pavilion, zen garden, must-see",
                    best_for: &["temples", "gardens", "iconic", "sightseeing"] },
                Area { id: "gion", name: "Gion", type_: "traditional",
                    stations: &["Gion-Shijo"],
                    vibe: "Geisha district, historic machiya, tea houses",
                    best_for: &["culture", "traditional", "geisha", "walking"] },
            ],
            pois: &[
                Poi { id: "bamboo_grove", title: "Arashiyama Bamboo Grove", area: "arashiyama",
                    nearest_station: "Arashiyama", duration_min: 60, booking_required: false,
                    booking_url: None, cost_estimate: 0,
                    tags: &["nature", "iconic", "walking", "photography"],
                    notes: Some("Arashiyama Bamboo Forest. Best at dawn or dusk. Combine with Tenryu-ji temple."),
                    hours: Some("24/7 (free), Tenryu-ji 08:00-17:00"), address: None, lat: None, lon: None },
                Poi { id: "tenryuji", title: "Tenryu-ji Temple", area: "arashiyama",
                    nearest_station: "Arashiyama", duration_min: 60, booking_required: false,
                    booking_url: None, cost_estimate: 500,
                    tags: &["temple", "garden", "zen", "unesco"],
                    notes: Some("UNESCO World Heritage. Stunning Zen garden. Less crowded than bamboo grove."),
                    hours: Some("08:00-17:00"), address: None, lat: None, lon: None },
                Poi { id: "fushimi_inari", title: "Fushimi Inari Taisha", area: "fushimi",
                    nearest_station: "Inari", duration_min: 180, booking_required: false,
                    booking_url: None, cost_estimate: 0,
                    tags: &["temple", "hiking", "iconic", "free"],
                    notes: Some("10,000+ torii gates. Hike to top takes 2-3 hours. Go early (6am) to avoid crowds."),
                    hours: Some("24/7"), address: None, lat: None, lon: None },
                Poi { id: "gion_walk", title: "Gion District Walk", area: "gion",
                    nearest_station: "Gion-Shijo", duration_min: 90, booking_required: false,
                    booking_url: None, cost_estimate: 0,
                    tags: &["walking", "traditional", "culture", "geisha"],
                    notes: Some("Historic merchant district. Look for maiko (apprentice geisha) in evening. Yasaka Pagoda nearby."),
                    hours: Some("Any time, evening for atmosphere"), address: None, lat: None, lon: None },
                Poi { id: "kiyomizudera", title: "Kiyomizu-dera Temple", area: "higashiyama",
                    nearest_station: "Kiyomizu-Gojo", duration_min: 90, booking_required: false,
                    booking_url: None, cost_estimate: 500,
                    tags: &["temple", "iconic", "views", "unesco"],
                    notes: Some("Wooden stage with panoramic Kyoto views. UNESCO site. Jishu shrine nearby for love luck."),
                    hours: Some("06:00-18:00"), address: None, lat: None, lon: None },
                Poi { id: "philosopher_path", title: "Philosopher's Path", area: "higashiyama",
                    nearest_station: "Kita-Oji", duration_min: 60, booking_required: false,
                    booking_url: None, cost_estimate: 0,
                    tags: &["walking", "nature", "cherry_blossoms", "quiet"],
                    notes: Some("2km stone path along canal. Cherry blossoms in early April. Connects to Ginkaku-ji."),
                    hours: Some("24/7"), address: None, lat: None, lon: None },
                Poi { id: "kinkakuji", title: "Kinkaku-ji (Golden Pavilion)", area: "kinkakuji",
                    nearest_station: "Kinkaji-mae", duration_min: 45, booking_required: false,
                    booking_url: None, cost_estimate: 500,
                    tags: &["temple", "iconic", "gardens", "unesco"],
                    notes: Some("Gold-leaf covered pavilion reflected in pond. Very popular, go early."),
                    hours: Some("09:00-17:00"), address: None, lat: None, lon: None },
                Poi { id: "nishiki_market", title: "Nishiki Market", area: "kyoto_station",
                    nearest_station: "Shijo", duration_min: 60, booking_required: false,
                    booking_url: None, cost_estimate: 2000,
                    tags: &["food", "market", "traditional", "ingredients"],
                    notes: Some("Kitchen of Kyoto. 400+ years of food stalls. Try yuba (tofu skin), tsukemono, sweets."),
                    hours: Some("09:00-17:00"), address: None, lat: None, lon: None },
            ],
            clusters: &[
                Cluster { id: "arashiyama_day", name: "Arashiyama Nature Day",
                    description: "Bamboo, temples, and mountains",
                    pois: &["bamboo_grove", "tenryuji", "philosopher_path"], duration_min: 240, best_area: "arashiyama" },
                Cluster { id: "kyoto_temple_run", name: "Essential Kyoto Temples",
                    description: "Classic Kyoto temple visits",
                    pois: &["kinkakuji", "kiyomizudera", "fushimi_inari"], duration_min: 360, best_area: "higashiyama" },
                Cluster { id: "kyoto_traditional", name: "Traditional Kyoto Walk",
                    description: "Gion, Higashiyama old town",
                    pois: &["gion_walk", "kiyomizudera", "nishiki_market"], duration_min: 300, best_area: "gion" },
            ],
            transit: &[
                Transit { pair_key: "kyoto_station_to_gion", kind: "estimate", minutes: 15,
                    line: "Keihan Line", station_from: None, station_to: None },
                Transit { pair_key: "kyoto_station_to_arashiyama", kind: "estimate", minutes: 15,
                    line: "JR Sagano Line", station_from: None, station_to: None },
                Transit { pair_key: "kyoto_station_to_fushimi_inari", kind: "estimate", minutes: 5,
                    line: "JR Nara Line", station_from: None, station_to: None },
            ],
            tips: &[
                "Kyoto bus pass (500 yen/day) covers most attractions",
                "Fushimi Inari opens 24/7 - go at sunrise (6am) to avoid crowds",
                "Gion most atmospheric at dusk - watch for maiko leaving teahouses",
            ],
        },
    ]
}

// ── run ──────────────────────────────────────────────────────────────────────

pub async fn run(args: &[String]) -> Result<(), String> {
    let _ = args;

    let conn = connect_write().await?;
    let dests = data();
    let fetched_at = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    let mut n_areas = 0usize;
    let mut n_pois = 0usize;
    let mut n_clusters = 0usize;
    let mut n_transit = 0usize;
    let mut n_tips = 0usize;
    let mut n_stmts = 0usize;

    for d in &dests {
        // areas
        for a in d.areas {
            conn.execute(
                "INSERT OR REPLACE INTO destination_areas (slug, area_id, name, type, vibe, source_url, fetched_at, confidence) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![d.slug, a.id, a.name, a.type_, a.vibe, SOURCE_URL, fetched_at.clone(), CONFIDENCE],
            ).await.map_err(|e| format!("destination_areas insert failed for {}/{}: {e}", d.slug, a.id))?;
            n_stmts += 1;

            // stations
            conn.execute("DELETE FROM destination_area_stations WHERE slug = ?1 AND area_id = ?2", params![d.slug, a.id])
                .await.map_err(|e| format!("delete area_stations failed: {e}"))?;
            for (i, station) in a.stations.iter().enumerate() {
                conn.execute(
                    "INSERT INTO destination_area_stations (slug, area_id, station, sort_order) VALUES (?1, ?2, ?3, ?4)",
                    params![d.slug, a.id, *station, i as i64],
                ).await.map_err(|e| format!("area_stations insert failed: {e}"))?;
                n_stmts += 1;
            }

            // best_for
            conn.execute("DELETE FROM destination_area_best_for WHERE slug = ?1 AND area_id = ?2", params![d.slug, a.id])
                .await.map_err(|e| format!("delete area_best_for failed: {e}"))?;
            for (i, tag) in a.best_for.iter().enumerate() {
                conn.execute(
                    "INSERT INTO destination_area_best_for (slug, area_id, tag, sort_order) VALUES (?1, ?2, ?3, ?4)",
                    params![d.slug, a.id, *tag, i as i64],
                ).await.map_err(|e| format!("area_best_for insert failed: {e}"))?;
                n_stmts += 1;
            }
            n_areas += 1;
        }

        // pois
        for p in d.pois {
            conn.execute(
                "INSERT OR REPLACE INTO destination_pois (slug, poi_id, title, area, nearest_station, duration_min, booking_required, booking_url, cost_estimate, notes, hours, address, source_url, fetched_at, confidence, lat, lon) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                params![d.slug, p.id, p.title, p.area, p.nearest_station, p.duration_min,
                    p.booking_required as i64, p.booking_url, p.cost_estimate,
                    p.notes, p.hours, p.address, SOURCE_URL, fetched_at.clone(), CONFIDENCE,
                    p.lat, p.lon],
            ).await.map_err(|e| format!("destination_pois insert failed for {}/{}: {e}", d.slug, p.id))?;
            n_stmts += 1;

            // tags
            conn.execute("DELETE FROM destination_poi_tags WHERE slug = ?1 AND poi_id = ?2", params![d.slug, p.id])
                .await.map_err(|e| format!("delete poi_tags failed: {e}"))?;
            for (i, tag) in p.tags.iter().enumerate() {
                conn.execute(
                    "INSERT INTO destination_poi_tags (slug, poi_id, tag, sort_order) VALUES (?1, ?2, ?3, ?4)",
                    params![d.slug, p.id, *tag, i as i64],
                ).await.map_err(|e| format!("poi_tags insert failed: {e}"))?;
                n_stmts += 1;
            }
            n_pois += 1;
        }

        // clusters
        for c in d.clusters {
            conn.execute(
                "INSERT OR REPLACE INTO destination_clusters (slug, cluster_id, name, description, duration_min, best_area, source_url, fetched_at, confidence) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![d.slug, c.id, c.name, c.description, c.duration_min, c.best_area,
                    SOURCE_URL, fetched_at.clone(), CONFIDENCE],
            ).await.map_err(|e| format!("destination_clusters insert failed for {}/{}: {e}", d.slug, c.id))?;
            n_stmts += 1;

            conn.execute("DELETE FROM destination_cluster_pois WHERE slug = ?1 AND cluster_id = ?2", params![d.slug, c.id])
                .await.map_err(|e| format!("delete cluster_pois failed: {e}"))?;
            for (i, poi_id) in c.pois.iter().enumerate() {
                conn.execute(
                    "INSERT INTO destination_cluster_pois (slug, cluster_id, poi_id, sort_order) VALUES (?1, ?2, ?3, ?4)",
                    params![d.slug, c.id, *poi_id, i as i64],
                ).await.map_err(|e| format!("cluster_pois insert failed: {e}"))?;
                n_stmts += 1;
            }
            n_clusters += 1;
        }

        // transit
        for t in d.transit {
            conn.execute(
                "INSERT OR REPLACE INTO destination_transit (slug, pair_key, kind, minutes, line, station_from, station_to, source_url, fetched_at, confidence) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![d.slug, t.pair_key, t.kind, t.minutes, t.line, t.station_from, t.station_to,
                    SOURCE_URL, fetched_at.clone(), CONFIDENCE],
            ).await.map_err(|e| format!("destination_transit insert failed for {}/{}: {e}", d.slug, t.pair_key))?;
            n_stmts += 1;
            n_transit += 1;
        }

        // tips
        conn.execute("DELETE FROM destination_tips WHERE slug = ?1", params![d.slug])
            .await.map_err(|e| format!("delete tips failed: {e}"))?;
        for (i, tip) in d.tips.iter().enumerate() {
            conn.execute(
                "INSERT INTO destination_tips (slug, tip, sort_order) VALUES (?1, ?2, ?3)",
                params![d.slug, *tip, i as i64],
            ).await.map_err(|e| format!("tips insert failed: {e}"))?;
            n_stmts += 1;
        }
        n_tips += d.tips.len();
    }

    println!("Seeded: {n_areas} areas, {n_pois} pois, {n_clusters} clusters, {n_transit} transit, {n_tips} tips across {} destinations.", dests.len());
    eprintln!("  ({n_stmts} statements executed)");
    Ok(())
}
