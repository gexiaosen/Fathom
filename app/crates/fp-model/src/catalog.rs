//! Built-in furniture catalogue: plain boxes with real dimensions.
//!
//! Everything ships with the app (no downloads), so a fresh install can furnish
//! a plan immediately.

use fp_core::{EntityId, Mm, Pt};

use crate::Furniture;

/// Coarse grouping used by the furniture library panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FurnitureCategory {
    /// Pseudo-category used by the library panel to list everything.
    All,
    Living,
    Bedroom,
    Dining,
    Bath,
    Appliance,
    Study,
}

impl FurnitureCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::Living => "客厅",
            Self::Bedroom => "卧室",
            Self::Dining => "餐厨",
            Self::Bath => "卫浴",
            Self::Appliance => "家电",
            Self::Study => "书房休闲",
        }
    }

    pub fn label_en(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Living => "Living",
            Self::Bedroom => "Bedroom",
            Self::Dining => "Dining",
            Self::Bath => "Bath",
            Self::Appliance => "Appliances",
            Self::Study => "Study",
        }
    }
}

pub const TABS: [FurnitureCategory; 7] = [
    FurnitureCategory::All,
    FurnitureCategory::Living,
    FurnitureCategory::Bedroom,
    FurnitureCategory::Dining,
    FurnitureCategory::Bath,
    FurnitureCategory::Appliance,
    FurnitureCategory::Study,
];

pub const CATEGORIES: [FurnitureCategory; 6] = [
    FurnitureCategory::Living,
    FurnitureCategory::Bedroom,
    FurnitureCategory::Dining,
    FurnitureCategory::Bath,
    FurnitureCategory::Appliance,
    FurnitureCategory::Study,
];

pub struct FurnitureSpec {
    pub asset_id: &'static str,
    pub name: &'static str,
    pub category: FurnitureCategory,
    pub width_mm: Mm,
    pub depth_mm: Mm,
    pub height_mm: Mm,
    /// Hex colour used by both the plan view and the 3D boxes.
    pub color: &'static str,
}

pub const CATALOG: &[FurnitureSpec] = &[
    FurnitureSpec {
        asset_id: "sofa",
        name: "三人沙发",
        category: FurnitureCategory::Living,
        width_mm: 2200,
        depth_mm: 950,
        height_mm: 800,
        color: "#8B6F55",
    },
    FurnitureSpec {
        asset_id: "sofa_two",
        name: "双人沙发",
        category: FurnitureCategory::Living,
        width_mm: 1600,
        depth_mm: 900,
        height_mm: 800,
        color: "#8B6F55",
    },
    FurnitureSpec {
        asset_id: "armchair",
        name: "单人扶手椅",
        category: FurnitureCategory::Living,
        width_mm: 900,
        depth_mm: 850,
        height_mm: 850,
        color: "#6B7A8F",
    },
    FurnitureSpec {
        asset_id: "coffee_table",
        name: "茶几",
        category: FurnitureCategory::Living,
        width_mm: 1200,
        depth_mm: 600,
        height_mm: 420,
        color: "#A07A4F",
    },
    FurnitureSpec {
        asset_id: "side_table",
        name: "边几",
        category: FurnitureCategory::Living,
        width_mm: 500,
        depth_mm: 500,
        height_mm: 550,
        color: "#9C8468",
    },
    FurnitureSpec {
        asset_id: "tv",
        name: "电视",
        category: FurnitureCategory::Living,
        width_mm: 1200,
        depth_mm: 100,
        height_mm: 700,
        color: "#3B3F48",
    },
    FurnitureSpec {
        asset_id: "tv_stand",
        name: "电视柜",
        category: FurnitureCategory::Living,
        width_mm: 1800,
        depth_mm: 400,
        height_mm: 500,
        color: "#5F6673",
    },
    FurnitureSpec {
        asset_id: "rug",
        name: "地毯",
        category: FurnitureCategory::Living,
        width_mm: 2400,
        depth_mm: 1600,
        height_mm: 15,
        color: "#BFAE9A",
    },
    FurnitureSpec {
        asset_id: "floor_lamp",
        name: "落地灯",
        category: FurnitureCategory::Living,
        width_mm: 350,
        depth_mm: 350,
        height_mm: 1600,
        color: "#D8D3C9",
    },
    FurnitureSpec {
        asset_id: "bookshelf",
        name: "书架",
        category: FurnitureCategory::Living,
        width_mm: 900,
        depth_mm: 350,
        height_mm: 2000,
        color: "#B7A58C",
    },
    FurnitureSpec {
        asset_id: "shoe_cabinet",
        name: "鞋柜",
        category: FurnitureCategory::Living,
        width_mm: 1000,
        depth_mm: 350,
        height_mm: 1100,
        color: "#A9A395",
    },
    FurnitureSpec {
        asset_id: "plant",
        name: "绿植",
        category: FurnitureCategory::Living,
        width_mm: 600,
        depth_mm: 600,
        height_mm: 1300,
        color: "#5E7F63",
    },
    FurnitureSpec {
        asset_id: "bed",
        name: "双人床",
        category: FurnitureCategory::Bedroom,
        width_mm: 1800,
        depth_mm: 2000,
        height_mm: 500,
        color: "#A9B3C4",
    },
    FurnitureSpec {
        asset_id: "bed_single",
        name: "单人床",
        category: FurnitureCategory::Bedroom,
        width_mm: 1200,
        depth_mm: 2000,
        height_mm: 500,
        color: "#A9B3C4",
    },
    FurnitureSpec {
        asset_id: "nightstand",
        name: "床头柜",
        category: FurnitureCategory::Bedroom,
        width_mm: 450,
        depth_mm: 400,
        height_mm: 550,
        color: "#B7A58C",
    },
    FurnitureSpec {
        asset_id: "wardrobe",
        name: "衣柜",
        category: FurnitureCategory::Bedroom,
        width_mm: 1200,
        depth_mm: 600,
        height_mm: 2200,
        color: "#B7A58C",
    },
    FurnitureSpec {
        asset_id: "wardrobe_wide",
        name: "推拉衣柜",
        category: FurnitureCategory::Bedroom,
        width_mm: 2000,
        depth_mm: 650,
        height_mm: 2400,
        color: "#C2B7A5",
    },
    FurnitureSpec {
        asset_id: "dresser",
        name: "梳妆台",
        category: FurnitureCategory::Bedroom,
        width_mm: 1000,
        depth_mm: 450,
        height_mm: 780,
        color: "#A9A395",
    },
    FurnitureSpec {
        asset_id: "bench_bed",
        name: "床尾凳",
        category: FurnitureCategory::Bedroom,
        width_mm: 1200,
        depth_mm: 400,
        height_mm: 450,
        color: "#8E8578",
    },
    FurnitureSpec {
        asset_id: "crib",
        name: "婴儿床",
        category: FurnitureCategory::Bedroom,
        width_mm: 1200,
        depth_mm: 700,
        height_mm: 900,
        color: "#D9D3C7",
    },
    FurnitureSpec {
        asset_id: "kids_desk",
        name: "儿童书桌",
        category: FurnitureCategory::Bedroom,
        width_mm: 900,
        depth_mm: 550,
        height_mm: 700,
        color: "#A9B3C4",
    },
    FurnitureSpec {
        asset_id: "mirror_stand",
        name: "化妆镜",
        category: FurnitureCategory::Bedroom,
        width_mm: 600,
        depth_mm: 150,
        height_mm: 1700,
        color: "#D8D3C9",
    },
    FurnitureSpec {
        asset_id: "dining_table",
        name: "餐桌",
        category: FurnitureCategory::Dining,
        width_mm: 1600,
        depth_mm: 900,
        height_mm: 750,
        color: "#A07A4F",
    },
    FurnitureSpec {
        asset_id: "round_table",
        name: "圆餐桌",
        category: FurnitureCategory::Dining,
        width_mm: 1200,
        depth_mm: 1200,
        height_mm: 750,
        color: "#A07A4F",
    },
    FurnitureSpec {
        asset_id: "chair",
        name: "餐椅",
        category: FurnitureCategory::Dining,
        width_mm: 450,
        depth_mm: 450,
        height_mm: 900,
        color: "#6B7A8F",
    },
    FurnitureSpec {
        asset_id: "bar_chair",
        name: "吧台椅",
        category: FurnitureCategory::Dining,
        width_mm: 400,
        depth_mm: 400,
        height_mm: 1050,
        color: "#5F6673",
    },
    FurnitureSpec {
        asset_id: "sideboard",
        name: "餐边柜",
        category: FurnitureCategory::Dining,
        width_mm: 1500,
        depth_mm: 450,
        height_mm: 900,
        color: "#B7A58C",
    },
    FurnitureSpec {
        asset_id: "kitchen_island",
        name: "中岛台",
        category: FurnitureCategory::Dining,
        width_mm: 1800,
        depth_mm: 800,
        height_mm: 900,
        color: "#D8D3C9",
    },
    FurnitureSpec {
        asset_id: "counter",
        name: "橱柜",
        category: FurnitureCategory::Dining,
        width_mm: 2000,
        depth_mm: 600,
        height_mm: 850,
        color: "#C8BDA8",
    },
    FurnitureSpec {
        asset_id: "sink_cabinet",
        name: "水槽柜",
        category: FurnitureCategory::Dining,
        width_mm: 1000,
        depth_mm: 600,
        height_mm: 850,
        color: "#D8D3C9",
    },
    FurnitureSpec {
        asset_id: "stove",
        name: "灶台",
        category: FurnitureCategory::Dining,
        width_mm: 750,
        depth_mm: 600,
        height_mm: 850,
        color: "#5F6673",
    },
    FurnitureSpec {
        asset_id: "dining_bench",
        name: "长凳",
        category: FurnitureCategory::Dining,
        width_mm: 1400,
        depth_mm: 350,
        height_mm: 450,
        color: "#9C8468",
    },
    FurnitureSpec {
        asset_id: "wine_cabinet",
        name: "酒柜",
        category: FurnitureCategory::Dining,
        width_mm: 600,
        depth_mm: 550,
        height_mm: 1800,
        color: "#5A4632",
    },
    FurnitureSpec {
        asset_id: "toilet",
        name: "马桶",
        category: FurnitureCategory::Bath,
        width_mm: 400,
        depth_mm: 700,
        height_mm: 750,
        color: "#E8EAEE",
    },
    FurnitureSpec {
        asset_id: "basin",
        name: "洗手盆柜",
        category: FurnitureCategory::Bath,
        width_mm: 800,
        depth_mm: 500,
        height_mm: 850,
        color: "#E8EAEE",
    },
    FurnitureSpec {
        asset_id: "bathtub",
        name: "浴缸",
        category: FurnitureCategory::Bath,
        width_mm: 1700,
        depth_mm: 800,
        height_mm: 550,
        color: "#E8EAEE",
    },
    FurnitureSpec {
        asset_id: "shower",
        name: "淋浴房",
        category: FurnitureCategory::Bath,
        width_mm: 900,
        depth_mm: 900,
        height_mm: 2000,
        color: "#CFD6DD",
    },
    FurnitureSpec {
        asset_id: "towel_rack",
        name: "毛巾架",
        category: FurnitureCategory::Bath,
        width_mm: 600,
        depth_mm: 150,
        height_mm: 1200,
        color: "#C9CED6",
    },
    FurnitureSpec {
        asset_id: "mirror_cab",
        name: "镜柜",
        category: FurnitureCategory::Bath,
        width_mm: 900,
        depth_mm: 150,
        height_mm: 800,
        color: "#CFD6DD",
    },
    FurnitureSpec {
        asset_id: "bath_mat",
        name: "浴室垫",
        category: FurnitureCategory::Bath,
        width_mm: 800,
        depth_mm: 500,
        height_mm: 15,
        color: "#B9C4CC",
    },
    FurnitureSpec {
        asset_id: "urinal",
        name: "小便器",
        category: FurnitureCategory::Bath,
        width_mm: 350,
        depth_mm: 400,
        height_mm: 900,
        color: "#E8EAEE",
    },
    FurnitureSpec {
        asset_id: "laundry_basket",
        name: "洗衣篮",
        category: FurnitureCategory::Bath,
        width_mm: 450,
        depth_mm: 450,
        height_mm: 600,
        color: "#B7A58C",
    },
    FurnitureSpec {
        asset_id: "fridge",
        name: "冰箱",
        category: FurnitureCategory::Appliance,
        width_mm: 700,
        depth_mm: 700,
        height_mm: 1800,
        color: "#C9CED6",
    },
    FurnitureSpec {
        asset_id: "fridge_double",
        name: "双门冰箱",
        category: FurnitureCategory::Appliance,
        width_mm: 900,
        depth_mm: 700,
        height_mm: 1900,
        color: "#C9CED6",
    },
    FurnitureSpec {
        asset_id: "washing_machine",
        name: "洗衣机",
        category: FurnitureCategory::Appliance,
        width_mm: 600,
        depth_mm: 600,
        height_mm: 850,
        color: "#E8EAEE",
    },
    FurnitureSpec {
        asset_id: "dryer",
        name: "烘干机",
        category: FurnitureCategory::Appliance,
        width_mm: 600,
        depth_mm: 600,
        height_mm: 850,
        color: "#E8EAEE",
    },
    FurnitureSpec {
        asset_id: "dishwasher",
        name: "洗碗机",
        category: FurnitureCategory::Appliance,
        width_mm: 600,
        depth_mm: 600,
        height_mm: 850,
        color: "#CFD6DD",
    },
    FurnitureSpec {
        asset_id: "microwave",
        name: "微波炉",
        category: FurnitureCategory::Appliance,
        width_mm: 500,
        depth_mm: 400,
        height_mm: 300,
        color: "#3B3F48",
    },
    FurnitureSpec {
        asset_id: "water_dispenser",
        name: "饮水机",
        category: FurnitureCategory::Appliance,
        width_mm: 350,
        depth_mm: 350,
        height_mm: 1000,
        color: "#C9CED6",
    },
    FurnitureSpec {
        asset_id: "air_purifier",
        name: "空气净化器",
        category: FurnitureCategory::Appliance,
        width_mm: 400,
        depth_mm: 400,
        height_mm: 700,
        color: "#E8EAEE",
    },
    FurnitureSpec {
        asset_id: "heater",
        name: "取暖器",
        category: FurnitureCategory::Appliance,
        width_mm: 500,
        depth_mm: 300,
        height_mm: 600,
        color: "#CFD6DD",
    },
    FurnitureSpec {
        asset_id: "range_hood",
        name: "油烟机",
        category: FurnitureCategory::Appliance,
        width_mm: 900,
        depth_mm: 600,
        height_mm: 600,
        color: "#C9CED6",
    },
    FurnitureSpec {
        asset_id: "ac",
        name: "空调挂机",
        category: FurnitureCategory::Appliance,
        width_mm: 900,
        depth_mm: 250,
        height_mm: 300,
        color: "#F2F3F5",
    },
    FurnitureSpec {
        asset_id: "desk",
        name: "书桌",
        category: FurnitureCategory::Study,
        width_mm: 1200,
        depth_mm: 600,
        height_mm: 750,
        color: "#9C8468",
    },
    FurnitureSpec {
        asset_id: "office_chair",
        name: "办公椅",
        category: FurnitureCategory::Study,
        width_mm: 600,
        depth_mm: 600,
        height_mm: 1100,
        color: "#3B3F48",
    },
    FurnitureSpec {
        asset_id: "file_cabinet",
        name: "文件柜",
        category: FurnitureCategory::Study,
        width_mm: 800,
        depth_mm: 400,
        height_mm: 1300,
        color: "#A9A395",
    },
    FurnitureSpec {
        asset_id: "lounge_chair",
        name: "休闲椅",
        category: FurnitureCategory::Study,
        width_mm: 800,
        depth_mm: 800,
        height_mm: 800,
        color: "#6B7A8F",
    },
    FurnitureSpec {
        asset_id: "piano",
        name: "钢琴",
        category: FurnitureCategory::Study,
        width_mm: 1500,
        depth_mm: 600,
        height_mm: 1200,
        color: "#2E2A28",
    },
    FurnitureSpec {
        asset_id: "treadmill",
        name: "跑步机",
        category: FurnitureCategory::Study,
        width_mm: 1800,
        depth_mm: 800,
        height_mm: 1400,
        color: "#3B3F48",
    },
    FurnitureSpec {
        asset_id: "cat_tree",
        name: "猫爬架",
        category: FurnitureCategory::Study,
        width_mm: 600,
        depth_mm: 600,
        height_mm: 1600,
        color: "#C8BDA8",
    },
    FurnitureSpec {
        asset_id: "chess_table",
        name: "棋牌桌",
        category: FurnitureCategory::Study,
        width_mm: 900,
        depth_mm: 900,
        height_mm: 750,
        color: "#9C8468",
    },
    FurnitureSpec {
        asset_id: "guitar_stand",
        name: "吉他架",
        category: FurnitureCategory::Study,
        width_mm: 400,
        depth_mm: 400,
        height_mm: 1200,
        color: "#8B6F55",
    },
    FurnitureSpec {
        asset_id: "corner_sofa",
        name: "转角沙发",
        category: FurnitureCategory::Living,
        width_mm: 2600,
        depth_mm: 1800,
        height_mm: 800,
        color: "#7C6248",
    },
    FurnitureSpec {
        asset_id: "bay_cushion",
        name: "飘窗垫",
        category: FurnitureCategory::Living,
        width_mm: 1800,
        depth_mm: 450,
        height_mm: 120,
        color: "#C9B79A",
    },
    FurnitureSpec {
        asset_id: "bean_bag",
        name: "懒人沙发",
        category: FurnitureCategory::Living,
        width_mm: 900,
        depth_mm: 900,
        height_mm: 700,
        color: "#8FA07C",
    },
    FurnitureSpec {
        asset_id: "water_heater",
        name: "热水器",
        category: FurnitureCategory::Appliance,
        width_mm: 500,
        depth_mm: 500,
        height_mm: 1200,
        color: "#D7DBDE",
    },
    FurnitureSpec {
        asset_id: "ac_wall",
        name: "壁挂空调",
        category: FurnitureCategory::Appliance,
        width_mm: 900,
        depth_mm: 220,
        height_mm: 300,
        color: "#E8EAEE",
    },
    FurnitureSpec {
        asset_id: "oven_column",
        name: "高柜烤箱",
        category: FurnitureCategory::Appliance,
        width_mm: 600,
        depth_mm: 600,
        height_mm: 2100,
        color: "#B9BEC7",
    },
];

impl Furniture {
    /// Instantiate a catalogue entry at a given centre and rotation.
    pub fn from_spec(id: EntityId, spec: &FurnitureSpec, center: Pt, rotation_deg: f64) -> Self {
        Self {
            id,
            asset_id: spec.asset_id.to_string(),
            name: spec.name.to_string(),
            center,
            width_mm: spec.width_mm,
            depth_mm: spec.depth_mm,
            rotation_deg,
            color: spec.color.to_string(),
            room_id: None,
        }
    }
}

/// Parse `#rrggbb` into linear-ish sRGB floats; invalid strings fall back grey.
pub fn hex_color(hex: &str) -> [f32; 3] {
    let s = hex.trim_start_matches('#');
    if s.len() != 6 {
        return [0.6, 0.6, 0.62];
    }
    let parse = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(150) as f32 / 255.0;
    [parse(0), parse(2), parse(4)]
}

/// Floor tint per material kind, shared by the plan and the 3D view.
pub fn material_kind_color(kind: &str) -> [f32; 3] {
    let k = kind.to_ascii_lowercase();
    if k.contains("walnut") {
        [0.58, 0.42, 0.30]
    } else if k.contains("wood") || k.contains("oak") {
        [0.77, 0.58, 0.39]
    } else if k.contains("marble") {
        [0.82, 0.81, 0.78]
    } else if k.contains("carpet") {
        [0.62, 0.55, 0.50]
    } else if k.contains("terrazzo") {
        [0.78, 0.76, 0.70]
    } else if k.contains("tile") || k.contains("ceramic") || k.contains("antislip") {
        [0.64, 0.69, 0.74]
    } else if k.contains("stone") {
        [0.70, 0.70, 0.68]
    } else {
        [0.71, 0.70, 0.77]
    }
}

#[test]
fn all_tab_lists_the_whole_catalogue() {
    assert_eq!(TABS[0].label(), "全部");
    for tab in TABS {
        if tab != FurnitureCategory::All {
            assert!(CATEGORIES.contains(&tab));
        }
    }
}

#[cfg(test)]
mod colour_tests {
    use super::material_kind_color;

    #[test]
    fn demo_kinds_are_distinct() {
        let wood = material_kind_color("walnut");
        let tile = material_kind_color("tile800");
        let terrazzo = material_kind_color("terrazzo");
        assert_ne!(wood, tile);
        assert_ne!(tile, terrazzo);
        assert_ne!(wood, material_kind_color("wood"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_usable_without_downloads() {
        assert!(CATALOG.len() >= 8);
        assert!(CATALOG.iter().all(|s| s.width_mm > 0 && s.depth_mm > 0));
    }

    #[test]
    fn hex_color_parses_and_falls_back() {
        assert_eq!(hex_color("#ffffff"), [1.0, 1.0, 1.0]);
        assert_eq!(hex_color("bogus"), [0.6, 0.6, 0.62]);
    }
    #[test]
    fn catalog_has_at_least_sixty_items() {
        assert!(CATALOG.len() >= 60, "only {}", CATALOG.len());
    }

    #[test]
    fn every_category_has_eight_items() {
        for category in CATEGORIES {
            let count = CATALOG.iter().filter(|s| s.category == category).count();
            assert!(count >= 8, "{:?} has {count}", category);
        }
    }

    #[test]
    fn catalog_ids_are_unique() {
        let mut ids: Vec<&str> = CATALOG.iter().map(|s| s.asset_id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before, "duplicate asset ids");
    }

    #[test]
    fn catalogue_entries_have_sane_dimensions() {
        for spec in CATALOG {
            assert!(
                spec.width_mm >= 100 && spec.width_mm <= 3000,
                "{}",
                spec.name
            );
            assert!(
                spec.depth_mm >= 100 && spec.depth_mm <= 3000,
                "{}",
                spec.name
            );
            assert!(
                spec.height_mm >= 10 && spec.height_mm <= 2500,
                "{}",
                spec.name
            );
        }
    }
}
