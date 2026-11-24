//! 地形上 GPU 的最后一跳（M2-C03b 运行时侧）：装配计划 → [`TerrainDrawItem`] → `Renderer::set_terrain`。
//!
//! # 为什么还要这一层，`terrain_scene` 不是已经装配好了吗
//!
//! [`crate::terrain_scene`] 给的是**每块一份**的网格（切块 / 选 LOD / 逐块生成），
//! 而 `TerrainPass` 认的是**一个**顶点缓冲 + 一个索引缓冲。块与几何之间这一步「拼」
//! 总要有人做，且必须同时看得见 `r3e_terrain`（块）与 `r3e_render`（绘制项）——
//! 按 `dep_rules`，只有运行时装配层两侧都够得着。
//!
//! # 判重（什么时候才值得重传）
//!
//! 一次上传就是几十万个 `f32` 过总线，绝不能每帧做。这里三档判据，任一成立才重传：
//!
//! * 装配层报 `rebuilds > 0` —— 某块 LOD 换了档（相机走近走远）；
//! * 地形数据的**结构或样本**变了（[`signature_of`] 与上一帧不同）—— 脚本挖了一口坑；
//! * 上一帧没有几何，这一帧有 —— 第一次出现。
//!
//! 三条都不成立时返回 [`TerrainUpload::Idle`]，调用方连 `set_terrain` 都不用调。
//!
//! # 为什么签名要抽样
//!
//! 高度样本按 2 米铺一张 200 米地面就是一万个 `f32`，逐帧全量哈希是纯烧 CPU。
//! 签名按 `len / 1024` 的步长抽样：哈希值与「整表哈希」不同，但只有
//! 「只改了一个样本、且那个样本恰好没被抽到」才会漏判——而这类编辑必然伴随
//! 尺寸 / 分辨率变化或下一次整体重铺，实测够用。抽样让签名的计算量与地图大小解耦。
//!
//! # 零地形零开销
//!
//! 场景里没有地形块时 [`TerrainDrawer::refresh`] 直接回 [`TerrainUpload::Idle`]
//! （上一帧也没地形）或 [`TerrainUpload::Clear`]（刚被删光），
//! 不建 grid、不生成网格、不碰 GPU。

use r3e_core::TerrainPatch;
use r3e_render::terrain::{TerrainDrawItem, TerrainVertex};

use crate::terrain_scene::{TerrainPlan, TerrainScene, TerrainSceneConfig};

/// 每块最多哈希多少个样本。
const SIGNATURE_SAMPLES: usize = 1024;

/// 相机位移多少米才值得重新装配一次（LOD 只在块级粒度上换档，
/// 逐米重算 16 块网格纯属浪费；8 米远小于最小 LOD 距离，肉眼无感）。
const REPLAN_EYE_STEP: f32 = 8.0;

/// FNV-1a 64 位偏移基数。
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64 位质数。
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// 一帧该对地形做什么。
#[derive(Clone, Debug, PartialEq)]
pub enum TerrainUpload {
    /// 无变化：不重传、不清通道。
    Idle,
    /// 地形没了：撤掉通道（释放管线与缓冲）。
    Clear,
    /// 有新几何：整份上传（顶点 + 索引一起换）。
    Item(TerrainDrawItem),
}

/// 地形 GPU 接线器。持有装配器与上一帧的判据快照。
#[derive(Debug)]
pub struct TerrainDrawer {
    scene: TerrainScene,
    /// 上一帧地形数据的签名。
    signature: u64,
    /// 上一帧是否已经把几何递给了渲染层。
    has_geometry: bool,
    /// 累计上传次数（观测用：用例可断言「相机静止的多帧只传一次」）。
    uploads: u64,
    /// 上次装配时的相机位置（`None` = 上次是 headless 装配）。
    last_eye: Option<[f32; 2]>,
}

impl Default for TerrainDrawer {
    fn default() -> Self {
        Self::new(TerrainSceneConfig::default())
    }
}

impl TerrainDrawer {
    pub fn new(config: TerrainSceneConfig) -> Self {
        Self {
            scene: TerrainScene::new(config),
            signature: signature_of(&[]),
            has_geometry: false,
            uploads: 0,
            last_eye: None,
        }
    }

    /// 累计上传次数。
    pub fn uploads(&self) -> u64 {
        self.uploads
    }

    /// 装配器（只读）。
    pub fn scene(&self) -> &TerrainScene {
        &self.scene
    }

    /// 装配器（可变）：改块尺寸 / 重配 LOD 距离走这里。
    pub fn scene_mut(&mut self) -> &mut TerrainScene {
        &mut self.scene
    }

    /// 当前是否已有几何在 GPU 侧。
    pub fn has_geometry(&self) -> bool {
        self.has_geometry
    }

    /// 算出这一帧要不要上传什么。
    ///
    /// `eye_xz` 是相机在 XZ 的位置，只用于选 LOD；给 `None`（headless / 离线烘焙）
    /// 时一律按 L0 全精度装配，产物可逐位对比。
    pub fn refresh(&mut self, patches: &[TerrainPatch], eye_xz: Option<[f32; 2]>) -> TerrainUpload {
        let signature = signature_of(patches);
        let changed = signature != self.signature;
        self.signature = signature;
        let had = self.has_geometry;

        if patches.is_empty() {
            // 让装配器也把块表清掉：否则地形回来时会被当成「同一批块」
            let _ = self.scene.plan(patches, eye_xz);
            self.last_eye = None;
            if had {
                self.has_geometry = false;
                return TerrainUpload::Clear;
            }
            return TerrainUpload::Idle;
        }

        if !changed && had && !self.eye_moved(eye_xz) {
            // 数据没变、相机也没动出阈值：连装配都不做（每帧重建 16 块网格是纯浪费）
            return TerrainUpload::Idle;
        }

        let plan = self.scene.plan(patches, eye_xz);
        self.last_eye = eye_xz;
        if plan.is_empty() {
            // 有 patch 却一块都没装配出来（配置非法 / 矩形退化）
            if had {
                self.has_geometry = false;
                return TerrainUpload::Clear;
            }
            return TerrainUpload::Idle;
        }

        if !changed && plan.rebuilds == 0 && had {
            // 相机动了但没跨过任何块的档位边界：几何一模一样，不重传
            return TerrainUpload::Idle;
        }

        let item = merge(&plan);
        if item.is_empty() {
            if had {
                self.has_geometry = false;
                return TerrainUpload::Clear;
            }
            return TerrainUpload::Idle;
        }
        self.has_geometry = true;
        self.uploads += 1;
        TerrainUpload::Item(item)
    }

    /// 相机是否移动出重规划阈值（含「从有相机变成没有相机」这类模式切换）。
    fn eye_moved(&self, eye_xz: Option<[f32; 2]>) -> bool {
        match (self.last_eye, eye_xz) {
            (Some(prev), Some(now)) => {
                let dx = now[0] - prev[0];
                let dz = now[1] - prev[1];
                (dx * dx + dz * dz).sqrt() >= REPLAN_EYE_STEP
            }
            // 模式切换：headless ↔ 有相机，装配口径不同（L0 vs 按距离），必须重算
            (None, Some(_)) | (Some(_), None) => true,
            (None, None) => false,
        }
    }
}

/// 一帧的地形同步：装配、判重、上传三件事都收在这里，`EngineApp` 只留一句调用。
///
/// 分成自由函数而不是 `EngineApp` 的方法，是因为它只需要「渲染器 + 接线器 +
/// 相机的 XZ」三样东西；放在 `app.rs` 里会让那个文件再长一截，而它的每一行
/// 都在 size_debt 白名单上被盯着（只许降不许涨）。
pub fn sync(
    renderer: &mut r3e_render::Renderer,
    drawer: &mut TerrainDrawer,
    eye_xz: [f32; 2],
    patches: &[TerrainPatch],
) {
    match drawer.refresh(patches, Some(eye_xz)) {
        TerrainUpload::Idle => {}
        TerrainUpload::Clear => renderer.set_terrain(None),
        TerrainUpload::Item(item) => renderer.set_terrain(Some(item)),
    }
}

/// 把多块网格拼成一份绘制项：顶点顺序拼接，索引整体平移。
///
/// 平移是必须的：每块的索引都从 0 起（块内局部），拼进同一个缓冲后必须加上
/// 该块顶点在合并数组里的起始偏移，否则第 2 块起会去索引第 1 块的顶点。
fn merge(plan: &TerrainPlan) -> TerrainDrawItem {
    let vertex_count: usize = plan.blocks.iter().map(|b| b.vertices.len()).sum();
    let index_count: usize = plan.blocks.iter().map(|b| b.indices.len()).sum();
    let mut vertices: Vec<TerrainVertex> = Vec::with_capacity(vertex_count);
    let mut indices: Vec<u32> = Vec::with_capacity(index_count);
    for block in &plan.blocks {
        let offset = vertices.len() as u32;
        vertices.extend_from_slice(&block.vertices);
        indices.extend(block.indices.iter().map(|i| i + offset));
    }
    TerrainDrawItem { vertices, indices }
}

/// 地形数据的签名：块数 + 每块的矩形 / 分辨率 / 样本（抽样）。
///
/// 只用 `to_bits`，不做浮点比较：签名要的是「逐位相同」，`-0.0` 与 `0.0`
/// 在几何上没有区别但在签名上是两回事——宁可多重传一次，也不要漏传。
fn signature_of(patches: &[TerrainPatch]) -> u64 {
    let mut h = FNV_OFFSET;
    fn eat(h: &mut u64, v: u64) {
        *h ^= v;
        *h = h.wrapping_mul(FNV_PRIME);
    }
    eat(&mut h, patches.len() as u64);
    for p in patches {
        for axis in 0..2 {
            eat(&mut h, p.origin[axis].to_bits() as u64);
            eat(&mut h, p.size[axis].to_bits() as u64);
            eat(&mut h, p.resolution[axis] as u64);
        }
        let len = p.heights.len();
        eat(&mut h, len as u64);
        if len > 0 {
            // 抽样：步长随表长放大，哈希次数封顶 SIGNATURE_SAMPLES
            let step = (len / SIGNATURE_SAMPLES).max(1);
            let mut i = 0;
            while i < len {
                eat(&mut h, p.heights[i].to_bits() as u64);
                i += step;
            }
            // 末样本必算：否则「只在尾部改」的一类编辑永远不进签名
            eat(&mut h, p.heights[len - 1].to_bits() as u64);
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_terrain::Lod;

    fn slab() -> TerrainPatch {
        TerrainPatch::flat("ground", [-100.0, -100.0], [200.0, 200.0])
    }

    /// 16 × 16 样本（默认分辨率为 1 × 1，看不出 LOD 差异，也放不下「改一个样本」）。
    fn hd_slab() -> TerrainPatch {
        let mut p = slab();
        p.resolution = [16, 16];
        p.heights = vec![0.0; 16 * 16];
        p
    }

    #[test]
    fn empty_scene_is_idle_not_clear() {
        let mut drawer = TerrainDrawer::default();
        assert_eq!(drawer.refresh(&[], None), TerrainUpload::Idle);
        assert_eq!(drawer.uploads(), 0);
        assert!(!drawer.has_geometry());
    }

    #[test]
    fn first_terrain_uploads_once() {
        let mut drawer = TerrainDrawer::default();
        let patches = [slab()];
        let TerrainUpload::Item(item) = drawer.refresh(&patches, None) else {
            panic!("第一次有地形必须上传");
        };
        assert!(!item.is_empty(), "上传的几何不能是空的");
        assert!(item.vertices.len() > 3, "顶点数 {}", item.vertices.len());
        assert_eq!(item.indices.len() % 3, 0, "索引必须是完整的三角形");
        assert!(drawer.has_geometry());
        assert_eq!(drawer.uploads(), 1);
    }

    #[test]
    fn same_terrain_same_eye_is_idle() {
        let mut drawer = TerrainDrawer::default();
        let patches = [slab()];
        assert!(matches!(
            drawer.refresh(&patches, Some([0.0, 0.0])),
            TerrainUpload::Item(_)
        ));
        for _ in 0..5 {
            assert_eq!(
                drawer.refresh(&patches, Some([0.0, 0.0])),
                TerrainUpload::Idle,
                "同一份地形、同一台相机，第二帧起不该再传"
            );
        }
        assert_eq!(drawer.uploads(), 1, "五帧静止只能有一次上传");
    }

    #[test]
    fn moving_camera_switch_lod_reuploads() {
        let mut drawer = TerrainDrawer::default();
        let patches = [slab()];
        assert!(matches!(
            drawer.refresh(&patches, Some([0.0, 0.0])),
            TerrainUpload::Item(_)
        ));
        // 拉到远处：块中心距离变大，至少有几块降档
        let far = drawer.refresh(&patches, Some([-4000.0, -4000.0]));
        assert!(
            matches!(far, TerrainUpload::Item(_)),
            "相机拉到远处必须因 LOD 变档而重传"
        );
        assert_eq!(drawer.uploads(), 2);
    }

    #[test]
    fn raising_the_ground_reuploads() {
        let mut drawer = TerrainDrawer::default();
        let flat = [hd_slab()];
        assert!(matches!(
            drawer.refresh(&flat, None),
            TerrainUpload::Item(_)
        ));
        // 抬高末样本：签名必变（末样本无条件入签名）
        let mut bumped = hd_slab();
        let last = bumped.heights.len() - 1;
        bumped.heights[last] = 12.5;
        assert!(
            matches!(drawer.refresh(&[bumped], None), TerrainUpload::Item(_)),
            "高度改了必须重传"
        );
        assert_eq!(drawer.uploads(), 2);
    }

    #[test]
    fn terrain_removal_clears_once() {
        let mut drawer = TerrainDrawer::default();
        let patches = [slab()];
        assert!(matches!(
            drawer.refresh(&patches, None),
            TerrainUpload::Item(_)
        ));
        assert_eq!(drawer.refresh(&[], None), TerrainUpload::Clear);
        assert!(!drawer.has_geometry());
        assert_eq!(
            drawer.refresh(&[], None),
            TerrainUpload::Idle,
            "撤过一次之后就没什么可撤的了"
        );
    }

    #[test]
    fn terrain_back_after_removal_uploads_again() {
        let mut drawer = TerrainDrawer::default();
        let patches = [slab()];
        assert!(matches!(
            drawer.refresh(&patches, None),
            TerrainUpload::Item(_)
        ));
        assert_eq!(drawer.refresh(&[], None), TerrainUpload::Clear);
        assert!(
            matches!(drawer.refresh(&patches, None), TerrainUpload::Item(_)),
            "地形去而复返必须重新上传"
        );
        assert_eq!(drawer.uploads(), 2);
    }

    #[test]
    fn merged_geometry_covers_every_block() {
        let mut drawer = TerrainDrawer::default();
        // 200 × 200 按默认 50 米切 = 4 × 4 = 16 块
        let patches = [slab()];
        let expected = drawer.scene_mut().plan(&patches, None).vertex_count();
        let TerrainUpload::Item(item) = drawer.refresh(&patches, None) else {
            panic!("必须上传");
        };
        assert_eq!(
            item.vertices.len(),
            expected,
            "合并后的顶点数必须等于各块之和"
        );
        // 索引全部落在顶点范围内（平移没错位）
        let max_index = item.indices.iter().copied().max().unwrap_or(0) as usize;
        assert!(max_index < item.vertices.len(), "索引越界：{max_index}");
        // 绕序：三角形法线一律朝上（否则地形背面剔除后看不见）
        let mut up = 0;
        for tri in item.indices.chunks_exact(3) {
            let a = item.vertices[tri[0] as usize].position;
            let b = item.vertices[tri[1] as usize].position;
            let c = item.vertices[tri[2] as usize].position;
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let ny = u[2] * v[0] - u[0] * v[2];
            if ny > 0.0 {
                up += 1;
            }
        }
        assert!(up > 0, "至少要有一个朝上的三角面");
        assert_eq!(
            up,
            item.indices.len() / 3,
            "全部三角面一律朝上（背面剔除后地形不能消失）"
        );
    }

    #[test]
    fn sign_is_stable_and_sensitive() {
        let a = signature_of(&[slab()]);
        assert_eq!(a, signature_of(&[slab()]), "同数据同签名");
        let mut moved = slab();
        moved.origin[0] = -99.0;
        assert_ne!(a, signature_of(&[moved]), "挪了矩形签名必须变");
        assert_ne!(a, signature_of(&[]), "空表签名不同");
    }

    #[test]
    fn lod_eyes_are_deterministic() {
        // 同一帧同一相机，两个接线器必须给出逐位相同的几何
        let patches = [slab()];
        let mut a = TerrainDrawer::default();
        let mut b = TerrainDrawer::default();
        let (TerrainUpload::Item(x), TerrainUpload::Item(y)) = (
            a.refresh(&patches, Some([12.0, 34.0])),
            b.refresh(&patches, Some([12.0, 34.0])),
        ) else {
            panic!("两边都必须上传");
        };
        assert_eq!(x, y, "同输入必须逐位相同");
    }

    #[test]
    fn headless_uses_l0() {
        let patches = [hd_slab()];
        let mut headless = TerrainDrawer::default();
        let mut far = TerrainDrawer::default();
        let TerrainUpload::Item(h) = headless.refresh(&patches, None) else {
            panic!("必须上传");
        };
        let TerrainUpload::Item(f) = far.refresh(&patches, Some([-4000.0, -4000.0])) else {
            panic!("必须上传");
        };
        assert!(
            h.vertices.len() > f.vertices.len(),
            "没有相机时必须按 L0 全精度（{} 顶点），远相机是降档（{} 顶点）",
            h.vertices.len(),
            f.vertices.len()
        );
        assert_eq!(Lod::L0.step(), 1);
    }
}
