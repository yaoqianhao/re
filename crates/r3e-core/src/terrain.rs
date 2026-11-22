//! 地形数据壳（M1-S3-C）。
//!
//! M1 阶段地形只做**数据承载**：一块地表 = 名字、世界矩形（origin/size）、高度样本网格、
//! 地表材质参数（albedo / roughness）。**不生成三角网、不做 LOD、不进渲染管线** ——
//! 几何求值与着色属 M2 的资产/渲染工作，这里先把「AI 能可靠写入、能回读、能算指纹」的
//! 契约钉死，避免后面接几何时再改一遍场景格式。
//!
//! 与 [`crate::LocalLight`] 同一套表语义（见 [`crate::light::upsert_local`]）：
//!
//! - `name` 是唯一主键，重复写同名 = 整体覆盖（不累积、顺序不变）；
//! - 上限 [`TERRAIN_MAX_PATCHES`] **只拦新增、不拦覆盖**，脚本每帧重发同一块地形必须能写进去；
//! - 入参一律经 [`TerrainPatch::sanitized`] 收敛，NaN / 越界 / 名字拼空格污染不了场景。

use serde::{Deserialize, Serialize};

use crate::material::{DEFAULT_ROUGHNESS, MIN_ROUGHNESS};

/// 地形块数量上限（只拦新增）。32 块足够拼一张几公里的关卡，也让哈希与快照有界。
pub const TERRAIN_MAX_PATCHES: usize = 32;

/// 单轴分辨率上限。超上限截断而不是拒绝：AI 写 512 时按 128 落盘，画面在 M2 之前无差别。
pub const TERRAIN_MAX_RESOLUTION: u32 = 128;

/// 地形块名字长度上限（字符数，按 `char` 边界截断）。
pub const TERRAIN_NAME_MAX_LEN: usize = 64;

/// 单轴尺寸下限（世界单位）。`0` 与负值都按缺省尺寸处理，避免零面积的块。
pub const TERRAIN_MIN_SIZE: f32 = 0.01;

fn default_size() -> [f32; 2] {
    [16.0, 16.0]
}

fn default_resolution() -> [u32; 2] {
    [1, 1]
}

fn default_albedo() -> [f32; 3] {
    [0.45, 0.42, 0.36]
}

/// 一块地表。
///
/// 字段一律用数组而非 `Vec2` / `Vec3`，与场景 JSON（`"size": [16, 16]`）严格对称，
/// 也不给 glam 加 serde feature —— 与 `LocalLight` / `UiElement` 保持同一约定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct TerrainPatch {
    /// 脚本 / RPC / 场景里的唯一主键。重复写同名 = 整体覆盖。
    pub name: String,
    /// 世界坐标下的矩形左下角（XZ 平面）。
    pub origin: [f32; 2],
    /// 世界单位下的矩形尺寸（XZ 平面），逐轴下限 [`TERRAIN_MIN_SIZE`]。
    pub size: [f32; 2],
    /// 高度样本网格的分辨率（列数, 行数），逐轴夹到 `1..=`[`TERRAIN_MAX_RESOLUTION`]。
    pub resolution: [u32; 2],
    /// 线性 RGB 反照率，逐通道夹到 `[0, 1]`。
    pub albedo: [f32; 3],
    /// 粗糙度，夹到 [`MIN_ROUGHNESS`]`..=1`。
    pub roughness: f32,
    /// 高度样本（行主序，长度 = `resolution.0 * resolution.1`）。
    ///
    /// **空数组表示平坦地面**（M1 的常见形态）；非空时按分辨率补齐 / 截断，
    /// 非有限值一律归零。M1 不高求值它，只保证「写进去什么、读出来什么」。
    pub heights: Vec<f32>,
}

impl Default for TerrainPatch {
    fn default() -> Self {
        Self {
            name: String::new(),
            origin: [0.0, 0.0],
            size: default_size(),
            resolution: default_resolution(),
            albedo: default_albedo(),
            roughness: DEFAULT_ROUGHNESS,
            heights: Vec::new(),
        }
    }
}

impl TerrainPatch {
    /// 新建一块平坦地表（其余字段走默认值）。
    pub fn flat(name: impl Into<String>, origin: [f32; 2], size: [f32; 2]) -> Self {
        Self {
            name: name.into(),
            origin,
            size,
            ..Self::default()
        }
    }

    /// 高度样本数应为 `resolution.0 * resolution.1`。
    pub fn expected_samples(&self) -> usize {
        self.resolution[0] as usize * self.resolution[1] as usize
    }

    /// 按分辨率取一个高度样本；坐标越界或样本缺失一律返回 `0.0`（平坦）。
    pub fn height_at(&self, x: u32, y: u32) -> f32 {
        if x >= self.resolution[0] || y >= self.resolution[1] {
            return 0.0;
        }
        let idx = y as usize * self.resolution[0] as usize + x as usize;
        self.heights.get(idx).copied().unwrap_or(0.0)
    }

    /// 块内双线性采样：世界坐标 `(x, z)` → 高度；点落在块外返回 `None`。
    ///
    /// 这条口径是 **M2-C01 从 `r3e-nav` 烘焙期上移上来的**（CD-3）：边界取整、
    /// 双线性权重顺序都与那边的私有实现逐位相同，所以 D 阶段删掉旧实现、
    /// 改成调这里时不会改变任何行为。`r3e-terrain` 的 `tests/sample.rs` 用
    /// 20 个点的交叉断言锁住这件事。
    ///
    /// 退化输入一律按「平坦地面」处理而不是报错：`heights` 空数组（合法表示）、
    /// 分辨率单轴 < 2（无法插值）、样本数与分辨率不符（数据已坏）都返回 `0.0`。
    /// 坐标非有限值返回 `None`——不 panic，也让「算不出高度」与「高度为 0」可区分。
    pub fn sample_height(&self, x: f32, z: f32) -> Option<f32> {
        let (w, d) = (self.size[0], self.size[1]);
        if !(w > 0.0 && d > 0.0) || !x.is_finite() || !z.is_finite() {
            return None;
        }
        let u = (x - self.origin[0]) / w;
        let v = (z - self.origin[1]) / d;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        let [rx, ry] = self.resolution;
        if self.heights.is_empty() || rx < 2 || ry < 2 {
            return Some(0.0);
        }
        if self.heights.len() != (rx * ry) as usize {
            return Some(0.0);
        }
        let fx = u * (rx - 1) as f32;
        let fz = v * (ry - 1) as f32;
        let x0 = fx.floor() as u32;
        let z0 = fz.floor() as u32;
        let x1 = (x0 + 1).min(rx - 1);
        let z1 = (z0 + 1).min(ry - 1);
        let tx = fx - x0 as f32;
        let tz = fz - z0 as f32;
        let h00 = self.height_at(x0, z0);
        let h10 = self.height_at(x1, z0);
        let h01 = self.height_at(x0, z1);
        let h11 = self.height_at(x1, z1);
        let a = h00 + (h10 - h00) * tx;
        let b = h01 + (h11 - h01) * tx;
        Some(a + (b - a) * tz)
    }

    /// 是否等于「新建一块平坦地表」的缺省形态（序列化时用来省掉冗余字段判断）。
    pub fn is_default_shape(&self) -> bool {
        self.size == default_size()
            && self.resolution == default_resolution()
            && self.albedo == default_albedo()
            && self.roughness == DEFAULT_ROUGHNESS
            && self.heights.is_empty()
    }

    /// 收敛到合法区间：名字去空白并截断、尺寸保正、分辨率夹紧、颜色/粗糙度夹紧、
    /// 高度样本对齐到 `resolution` 长度且不含 NaN。
    ///
    /// 幂等（收敛两次结果相同）—— 命令层与加载层都会调用，幂等才不会互相打架。
    pub fn sanitized(mut self) -> Self {
        self.name = self.name.trim().to_string();
        self.name = truncate_chars(&self.name, TERRAIN_NAME_MAX_LEN);

        self.origin = [
            finite_or(self.origin[0], 0.0),
            finite_or(self.origin[1], 0.0),
        ];

        let size = default_size();
        self.size = [
            positive_or(self.size[0], size[0]),
            positive_or(self.size[1], size[1]),
        ];

        self.resolution = [
            self.resolution[0].clamp(1, TERRAIN_MAX_RESOLUTION),
            self.resolution[1].clamp(1, TERRAIN_MAX_RESOLUTION),
        ];

        let albedo = default_albedo();
        for (slot, fallback) in self.albedo.iter_mut().zip(albedo) {
            *slot = match *slot {
                v if v.is_finite() => v.clamp(0.0, 1.0),
                _ => fallback,
            };
        }

        self.roughness = match self.roughness {
            v if v.is_finite() => v.clamp(MIN_ROUGHNESS, 1.0),
            _ => DEFAULT_ROUGHNESS,
        };

        if !self.heights.is_empty() {
            let want = self.expected_samples();
            self.heights.truncate(want);
            self.heights.resize(want, 0.0);
            for h in &mut self.heights {
                if !h.is_finite() {
                    *h = 0.0;
                }
            }
        }

        self
    }
}

fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

fn positive_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() && v >= TERRAIN_MIN_SIZE {
        v
    } else {
        fallback
    }
}

/// 按 `char` 边界截断到 `max` 个字符（不切坏 UTF-8）。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

/// 场景层面的地形表操作：按 name 覆盖式写入（同名原地替换、顺序不变）。
///
/// 与 [`crate::light::upsert_local`] 同一套语义：**上限只拦新增、不拦覆盖**。
/// 返回 `true` 表示写入生效（新建或覆盖），`false` 表示表已满且是新名字。
pub fn upsert_patch(patches: &mut Vec<TerrainPatch>, patch: TerrainPatch) -> bool {
    let patch = patch.sanitized();
    if patch.name.is_empty() {
        return false;
    }
    if let Some(slot) = patches.iter_mut().find(|p| p.name == patch.name) {
        *slot = patch;
        return true;
    }
    if patches.len() >= TERRAIN_MAX_PATCHES {
        return false;
    }
    patches.push(patch);
    true
}

/// 按 name 移除；返回是否真的删掉了。
pub fn remove_patch(patches: &mut Vec<TerrainPatch>, name: &str) -> bool {
    match patches.iter().position(|p| p.name == name) {
        Some(idx) => {
            patches.remove(idx);
            true
        }
        None => false,
    }
}

/// 按 name 查一块地形。
pub fn find_patch<'a>(patches: &'a [TerrainPatch], name: &str) -> Option<&'a TerrainPatch> {
    patches.iter().find(|p| p.name == name)
}

/// 把加载来的地形表整体收敛：丢空名、同名后者覆盖前者、超上限截断。
pub fn normalize_all(patches: Vec<TerrainPatch>) -> Vec<TerrainPatch> {
    let mut out: Vec<TerrainPatch> = Vec::new();
    for patch in patches {
        if patch.name.trim().is_empty() {
            continue;
        }
        let _ = upsert_patch(&mut out, patch);
    }
    out
}
