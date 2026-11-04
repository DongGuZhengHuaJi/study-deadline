//! Small shared platform metadata that is not part of an adapter package.
//!
//! Adapter logic lives in the QuickJS packages under `adapters/`. This module
//! only keeps constants and the Yuketang region list that the host UI needs
//! before/around adapter execution.

use crate::models::RegionInfo;

pub const YUKETANG_ADAPTER_ID: &str = "yuketang";

pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/139.0.0.0 Safari/537.36";

const YUKETANG_REGIONS: [(&str, &str, &str); 4] = [
    ("www", "标准雨课堂", "https://www.yuketang.cn"),
    ("pro", "荷塘雨课堂", "https://pro.yuketang.cn"),
    (
        "changjiang",
        "长江雨课堂",
        "https://changjiang.yuketang.cn",
    ),
    ("huanghe", "黄河雨课堂", "https://huanghe.yuketang.cn"),
];

pub fn yuketang_regions() -> Vec<RegionInfo> {
    YUKETANG_REGIONS
        .iter()
        .map(|(id, label, base_url)| RegionInfo {
            id: id.to_string(),
            label: label.to_string(),
            base_url: base_url.to_string(),
        })
        .collect()
}

pub fn yuketang_region(id: &str) -> Option<RegionInfo> {
    yuketang_regions().into_iter().find(|region| region.id == id)
}
