//! 字形图集分配器：把一张张字形位图塞进若干张「页」里，并按字形键复用。
//!
//! 这里只有纯 CPU 的几何记账 —— 谁在图集的哪一页、哪个位置、占多大，
//! 以及页满了以后怎么长出新页。真正的纹理对象、上传指令都属于渲染层
//! （`r3e-render`），所以本模块不依赖任何图形 API。
//!
//! 分配策略是最朴素的 **shelf（货架）法**：每页按行向下堆，一行放不下就另起一行。
//! 对字形这种「高度按字号聚类、宽度参差」的内容，货架法命中率高、实现小、碎片可控
//! （同一字号的行高一致，不会出现行内吃不满导致的纵向浪费）。

use std::collections::HashMap;

/// 默认图集页边长（像素）。1024² 单通道能放约 1000 个 32px 汉字，足够一屏 HUD。
pub const DEFAULT_PAGE_SIZE: u16 = 1024;
/// 默认最多允许多少页。超出后新字形会被拒绝绘制（不会 panic，也不会挤坏已有页）。
pub const DEFAULT_MAX_PAGES: u16 = 4;
/// 图集页边长下限 / 上限。
pub const MIN_PAGE_SIZE: u16 = 128;
pub const MAX_PAGE_SIZE: u16 = 4096;
/// glyph 之间留 1px 间隙，避免线性采样时相邻字形互相渗色。
const PADDING: u16 = 1;

/// 字形缓存的键：字体面 + 字形索引 + 量化后的像素字号。
///
/// 字号用 1/16 像素量化（`px_q`），这样 `32.0` 与 `32.01` 复用同一份位图，
/// 避免脚本每帧传浮点抖动把图集撑爆。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub face: u16,
    pub glyph: u16,
    pub px_q: u16,
}

impl GlyphKey {
    pub fn new(face: u16, glyph: u16, px: f32) -> Self {
        Self {
            face,
            glyph,
            px_q: quantize_px(px),
        }
    }
}

/// 把字号量化到 1/16 像素，并夹到 `u16` 可表示范围内。
pub fn quantize_px(px: f32) -> u16 {
    let px = if px.is_finite() { px } else { 0.0 };
    (px.clamp(0.0, 4000.0) * 16.0).round() as u16
}

/// 一个字形落在图集里的位置。
///
/// `left` / `top` 是相对笔位置 / 基线的偏移（位图左上角），单位像素；
/// `top` 已换算成「基线之上为负」的屏幕坐标习惯，见 `r3e-render` 的四边形拼装。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasSlot {
    pub page: u16,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub left: i16,
    pub top: i16,
}

/// 插入结果：复用已有槽位，还是刚分配了新槽位（新槽位才需要上传纹理）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtlasInsert {
    Reused(AtlasSlot),
    Allocated(AtlasSlot),
}

impl AtlasInsert {
    pub fn slot(self) -> AtlasSlot {
        match self {
            AtlasInsert::Reused(s) | AtlasInsert::Allocated(s) => s,
        }
    }

    pub fn is_new(self) -> bool {
        matches!(self, AtlasInsert::Allocated(_))
    }
}

/// 一层货架。`x` 是当前行已用宽度，`y` 是行顶，`height` 是行高。
#[derive(Debug, Clone, Copy)]
struct Shelf {
    y: u16,
    height: u16,
    x: u16,
}

/// 单页货架分配器。
#[derive(Debug, Clone)]
pub struct ShelfAllocator {
    width: u16,
    height: u16,
    shelves: Vec<Shelf>,
    used_area: u64,
}

impl ShelfAllocator {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width: width.max(1),
            height: height.max(1),
            shelves: Vec::new(),
            used_area: 0,
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// 已用像素面积（不含 padding），用于观察图集利用率。
    pub fn used_area(&self) -> u64 {
        self.used_area
    }

    pub fn is_empty(&self) -> bool {
        self.shelves.is_empty()
    }

    /// 已用高度（最后一行的底部），调试 / 统计用。
    pub fn used_height(&self) -> u16 {
        self.shelves
            .last()
            .map(|s| s.y.saturating_add(s.height))
            .unwrap_or(0)
    }

    pub fn reset(&mut self) {
        self.shelves.clear();
        self.used_area = 0;
    }

    /// 申请 `w × h` 的矩形，返回左上角坐标；放不下返回 `None`。
    pub fn allocate(&mut self, w: u16, h: u16) -> Option<(u16, u16)> {
        if w == 0 || h == 0 || w > self.width || h > self.height {
            return None;
        }
        // 1) 先尝试塞进已有行
        for shelf in &mut self.shelves {
            if shelf.height < h {
                continue;
            }
            let right = shelf.x.saturating_add(w);
            if right <= self.width {
                let x = shelf.x;
                let y = shelf.y;
                shelf.x = right.saturating_add(PADDING);
                self.used_area += u64::from(w) * u64::from(h);
                return Some((x, y));
            }
        }
        // 2) 另起一行
        let y = self
            .shelves
            .last()
            .map(|s| s.y.saturating_add(s.height).saturating_add(PADDING))
            .unwrap_or(0);
        if y.saturating_add(h) > self.height {
            return None;
        }
        self.shelves.push(Shelf {
            y,
            height: h,
            x: w.saturating_add(PADDING),
        });
        self.used_area += u64::from(w) * u64::from(h);
        Some((0, y))
    }
}

/// 多页字形图集：`GlyphKey -> AtlasSlot` 的查找表 + 每页的货架分配器。
///
/// 同一个键重复插入是幂等的（直接复用旧槽位），这保证了「每帧都可能请求同一批字形」
/// 的场景下不会无限增长；页放满时会自动追加新页，直到 `max_pages` 为止。
#[derive(Debug, Clone)]
pub struct GlyphAtlas {
    page_size: u16,
    max_pages: u16,
    pages: Vec<ShelfAllocator>,
    slots: HashMap<GlyphKey, AtlasSlot>,
}

impl Default for GlyphAtlas {
    fn default() -> Self {
        Self::new(DEFAULT_PAGE_SIZE, DEFAULT_MAX_PAGES)
    }
}

impl GlyphAtlas {
    pub fn new(page_size: u16, max_pages: u16) -> Self {
        let page_size = page_size.clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE);
        let max_pages = max_pages.clamp(1, 16);
        Self {
            page_size,
            max_pages,
            pages: vec![ShelfAllocator::new(page_size, page_size)],
            slots: HashMap::new(),
        }
    }

    pub fn page_size(&self) -> u16 {
        self.page_size
    }

    pub fn max_pages(&self) -> u16 {
        self.max_pages
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn page(&self, index: usize) -> Option<&ShelfAllocator> {
        self.pages.get(index)
    }

    /// 已缓存的字形数量。
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn get(&self, key: &GlyphKey) -> Option<AtlasSlot> {
        self.slots.get(key).copied()
    }

    pub fn contains(&self, key: &GlyphKey) -> bool {
        self.slots.contains_key(key)
    }

    /// 清空全部槽位（保留页数，便于复用时少一次 GPU 重建）。
    pub fn clear(&mut self) {
        self.slots.clear();
        for page in &mut self.pages {
            page.reset();
        }
    }

    /// 为 `key` 申请 `w × h` 的槽位，并记录位图的 bearing。
    ///
    /// 返回 `None` 表示所有页都放不下（页数已达上限或单个字形比整页还大）。
    pub fn insert(
        &mut self,
        key: GlyphKey,
        w: u16,
        h: u16,
        left: i16,
        top: i16,
    ) -> Option<AtlasInsert> {
        if let Some(slot) = self.slots.get(&key) {
            return Some(AtlasInsert::Reused(*slot));
        }
        for (index, page) in self.pages.iter_mut().enumerate() {
            if let Some((x, y)) = page.allocate(w, h) {
                let slot = AtlasSlot {
                    page: index as u16,
                    x,
                    y,
                    w,
                    h,
                    left,
                    top,
                };
                self.slots.insert(key, slot);
                return Some(AtlasInsert::Allocated(slot));
            }
        }
        if self.pages.len() < self.max_pages as usize {
            let mut page = ShelfAllocator::new(self.page_size, self.page_size);
            let allocated = page.allocate(w, h);
            self.pages.push(page);
            let index = self.pages.len() - 1;
            if let Some((x, y)) = allocated {
                let slot = AtlasSlot {
                    page: index as u16,
                    x,
                    y,
                    w,
                    h,
                    left,
                    top,
                };
                self.slots.insert(key, slot);
                return Some(AtlasInsert::Allocated(slot));
            }
        }
        None
    }

    /// 图集整体利用率（已用像素 / 全部页面积），0..1。
    pub fn occupancy(&self) -> f32 {
        let total = u64::from(self.page_size) * u64::from(self.page_size) * self.pages.len() as u64;
        if total == 0 {
            return 0.0;
        }
        let used: u64 = self.pages.iter().map(|p| p.used_area()).sum();
        used as f32 / total as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(glyph: u16, px: f32) -> GlyphKey {
        GlyphKey::new(0, glyph, px)
    }

    #[test]
    fn same_key_reuses_slot() {
        let mut atlas = GlyphAtlas::new(256, 2);
        let k = key(42, 32.0);

        let first = atlas.insert(k, 24, 30, 1, -28).expect("首次分配应成功");
        assert!(first.is_new(), "首次应当是新建槽位");

        let again = atlas.insert(k, 24, 30, 1, -28).expect("重复分配应复用");
        matches!(again, AtlasInsert::Reused(_));
        assert_eq!(first.slot(), again.slot(), "同一字形必须落在同一槽位");
        assert_eq!(atlas.len(), 1, "重复请求不得增加缓存条目");
        assert_eq!(atlas.page_count(), 1, "重复请求不得新开页");
    }

    #[test]
    fn size_quantization_shares_bitmap() {
        // 32.0 与 32.01 量化后同键 —— 脚本浮点抖动不该把图集撑爆
        assert_eq!(quantize_px(32.0), quantize_px(32.01));
        // 32.0 与 33.0 必须分开，否则不同字号会串味
        assert_ne!(quantize_px(32.0), quantize_px(33.0));
        // 负数 / NaN / 超大值都要收敛到合法范围
        assert_eq!(quantize_px(-5.0), 0);
        assert_eq!(quantize_px(f32::NAN), 0);
        assert!(quantize_px(1.0e9) > 0);
    }

    #[test]
    fn different_glyphs_get_distinct_slots() {
        let mut atlas = GlyphAtlas::new(256, 2);
        let a = atlas.insert(key(1, 24.0), 20, 24, 0, -22).expect("a");
        let b = atlas.insert(key(2, 24.0), 20, 24, 0, -22).expect("b");
        assert_ne!(a.slot(), b.slot(), "不同字形不能落在同一槽位");
        assert_eq!(atlas.len(), 2);
    }

    #[test]
    fn page_grows_when_full() {
        // 128² 的页，每次占 60×60：一页最多 2 行 × 2 列 = 4 个，第 5 个必须长出新页
        let mut atlas = GlyphAtlas::new(128, 3);
        for gid in 0..4u16 {
            let ins = atlas
                .insert(key(gid, 40.0), 60, 60, 0, 0)
                .expect("前 4 个应放得下");
            assert_eq!(ins.slot().page, 0, "前 4 个应仍在第 0 页");
        }
        let ins = atlas
            .insert(key(99, 40.0), 60, 60, 0, 0)
            .expect("第 5 个应长出新页");
        assert!(ins.is_new());
        assert_eq!(ins.slot().page, 1, "放不下时必须长出新页");
        assert_eq!(atlas.page_count(), 2);
    }

    #[test]
    fn refuses_when_pages_exhausted() {
        // 只允许 1 页且每页只放得下一个 100×100：第二个必须被拒绝而不是 panic
        let mut atlas = GlyphAtlas::new(128, 1);
        assert!(atlas.insert(key(1, 40.0), 100, 100, 0, 0).is_some());
        assert!(
            atlas.insert(key(2, 40.0), 100, 100, 0, 0).is_none(),
            "页数与面积都耗尽时必须返回 None"
        );
        assert_eq!(atlas.len(), 1, "被拒绝的字形不得进缓存");
    }

    #[test]
    fn oversize_glyph_is_rejected() {
        let mut atlas = GlyphAtlas::new(128, 2);
        assert!(
            atlas.insert(key(1, 256.0), 200, 20, 0, 0).is_none(),
            "比整页还宽的字形必须直接拒绝"
        );
        assert!(atlas.insert(key(2, 256.0), 20, 200, 0, 0).is_none());
    }

    #[test]
    fn shelf_allocator_packs_rows() {
        let mut page = ShelfAllocator::new(64, 64);
        assert_eq!(page.allocate(30, 10), Some((0, 0)));
        let second = page.allocate(30, 10).expect("同一行应还能放一个");
        assert!(second.0 >= 30, "第二个必须排到右边");
        assert_eq!(second.1, 0, "同一行 y 必须一致");
        let third = page.allocate(30, 10).expect("应另起一行");
        assert_eq!(third.0, 0);
        assert!(third.1 >= 10, "第三行必须在第二行之下");
        assert!(page.used_height() > 0);
        page.reset();
        assert!(page.is_empty());
        assert_eq!(
            page.allocate(30, 10),
            Some((0, 0)),
            "reset 后应从原点重新分配"
        );
    }

    #[test]
    fn occupancy_tracks_usage() {
        let mut atlas = GlyphAtlas::new(128, 2);
        assert_eq!(atlas.occupancy(), 0.0);
        atlas.insert(key(1, 20.0), 64, 64, 0, 0).expect("分配");
        let occ = atlas.occupancy();
        assert!(occ > 0.0 && occ < 1.0, "占用率应在 (0,1) 之间，实际 {occ}");
        atlas.clear();
        assert!(atlas.is_empty());
        assert_eq!(atlas.occupancy(), 0.0);
    }
}
