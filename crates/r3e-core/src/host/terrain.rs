//! 地形派生几何能力面：脚本看到的「块划分 / 采样 / 重采样 / 生成」（M2-C06）。
//!
//! 与 [`PhysicsHost`](super::physics::PhysicsHost) /
//! [`CharacterHost`](super::character::CharacterHost) 同一套路：`r3e-core` 只声明
//! **诉求 / 应答 / 默认方法**，真实现落在 `r3e-runtime`（`r3e-terrain` 的纯函数
//! 只能在那边用），由装配层在构造 `ScriptHost` 时注入。这样 `r3e-script` 仍然
//! 不新增对 `r3e-terrain` 的依赖边（`xtask/dep_rules.json` 的 allow 保持不变）。
//!
//! ## 为什么必须是能力面而不是「脚本自己算」
//!
//! 脚本侧有七条 `terrain_*` 命令要交付结果，其中 `terrain_gen` / `terrain_resample`
//! 会**写场景**，`terrain_sample` 要回四层权重。这些算法（程序化噪声、LOD 抽稀、
//! 层权重判定）全在 `r3e-terrain`，脚本自己重写一份的结果必然与 RPC 侧
//! `terrain.*` 字节级不同——两套真值正是 CD-2 要禁的那类漂移。
//!
//! ## 三条设计约束（改本文件前先读）
//!
//! 1. **写场景不在这里**：本 trait 只做**纯计算**。`create` / `remove` / `gen` 的落笔
//!    由脚本侧走 `r3e_doc::CommandKind`（`r3e-script` 本来就依赖 `r3e-doc`），
//!    所以 `create` / `remove` 两条**不需要**任何能力面方法，`gen` / `resample`
//!    只要求「算出要写什么」。
//! 2. **DTO 全是脚本友好类型**：`f64` / `[u32; 2]` / `Vec<String>` / 扁平常量，
//!    不出现 `glam` 类型、不出现 `serde_json::Value`（序列化归脚本 API 层）。
//! 3. **没接后端一律「没有」，不 panic**：默认方法与 [`NullTerrainHost`] 同一口径，
//!    脚本侧据此打印提示并返回 `false`，流程不断。

use std::any::Any;

use crate::terrain::TerrainPatch;

/// 一次世界点采样的结果（口径同 `terrain.sample` 的 RPC 回包）。
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainSampleDto {
    /// 命中的地形块名；没命中为空串。
    pub name: String,
    /// 世界点高度（米）。`inside = false` 时为 `0.0`。
    pub height: f64,
    /// 归一化法线（Y 为上）。`inside = false` 时为 `[0, 1, 0]`。
    pub normal: [f64; 3],
    /// 点是否落在这块地形矩形内。**「没有地形」≠「地形是平的」**。
    pub inside: bool,
    /// 四层混合权重，和为 1。
    pub weights: [f64; 4],
}

impl Default for TerrainSampleDto {
    fn default() -> Self {
        Self {
            name: String::new(),
            height: 0.0,
            normal: [0.0, 1.0, 0.0],
            inside: false,
            weights: [1.0, 0.0, 0.0, 0.0],
        }
    }
}

/// 一块划分的信息（块索引 + 世界矩形 + 覆盖它的地形块名）。
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainBlockDto {
    /// 块索引 `[列, 行]`。
    pub index: [u32; 2],
    /// 世界矩形 `[min_x, min_z, max_x, max_z]`，已裁到整张地形范围。
    pub rect: [f64; 4],
    /// 与这块矩形有交集的地形块名（空 = 这块是空白）。
    pub patches: Vec<String>,
}

/// 一次块划分的结果（口径同 `terrain.blocks`）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TerrainBlocksDto {
    /// 整张地形的世界矩形；空地形为全零。
    pub rect: [f64; 4],
    /// 块边长（米）。
    pub block_size: f64,
    /// 块数 `[列数, 行数]`；空地形为 `[0, 0]`。
    pub dims: [u32; 2],
    /// 总块数。
    pub count: u32,
    /// 有地形覆盖的块数。
    pub covered: u32,
    /// 逐块清单（行主序）。
    pub blocks: Vec<TerrainBlockDto>,
}

/// 地形汇总（口径同 `terrain.stats`，三角数与 `terrain.mesh` 同源）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TerrainStatsDto {
    /// 地形块数。
    pub patches: usize,
    /// 高度样本总数。
    pub samples: usize,
    /// 切出来的总块数。
    pub blocks: u32,
    /// 有地形覆盖的块数。
    pub covered: u32,
    /// 全分辨率三角数。
    pub triangles: usize,
    /// 全分辨率顶点数。
    pub vertices: usize,
    /// 统计用的顶点步长倍数（1 = 全分辨率）。
    pub lod_step: f64,
    /// 切块用的块边长（米）。
    pub block_size: f64,
}

/// 一次重采样的结果：**算出来的新网格**，落笔仍由脚本侧做。
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainResampleDto {
    /// 重采样前的分辨率。
    pub from: [u32; 2],
    /// 重采样后的分辨率。
    pub to: [u32; 2],
    /// 新网格的样本数。
    pub samples: usize,
    /// 同一批世界点上新旧网格的最大高度差（米）。
    pub max_error: f64,
    /// 重采样后的完整地形块（分辨率与 `heights` 已更新）。
    pub patch: TerrainPatch,
    /// 是否真的改动了网格。平坦块（`heights` 空）为 `false` —— 写一条内容相同的
    /// 命令只会白占一格撤销栈，脚本侧据此跳过落笔。
    pub changed: bool,
}

/// 脚本对地形派生几何的全部诉求。实现方只需给出 `execute`。
#[derive(Debug)]
pub enum TerrainReq<'a> {
    /// 世界点采样。`name` 为 `None` 时跨全部块取最高层。
    Sample {
        /// 场景当前的全部地形块（调用方借用，实现方不得长期持有）。
        patches: &'a [TerrainPatch],
        /// 只在指定的块里采样。
        name: Option<&'a str>,
        /// 世界坐标 X / Z（米）。
        x: f64,
        /// 世界坐标 Z（米）。
        z: f64,
    },
    /// 按块边长切块并回逐块清单。`block_size` 为 `None` 时用缺省块边长。
    Blocks {
        /// 场景当前的全部地形块。
        patches: &'a [TerrainPatch],
        /// 块边长（米）。
        block_size: Option<f64>,
    },
    /// 汇总统计（块数 / 样本数 / 三角数）。
    Stats {
        /// 场景当前的全部地形块。
        patches: &'a [TerrainPatch],
    },
    /// 按 LOD 步长重采样一块地形（只算，不写场景）。
    Resample {
        /// 待重采样的地形块。
        patch: &'a TerrainPatch,
        /// 顶点步长倍数（1 / 2 / 4，其余值由实现方就近收敛）。
        step: f64,
        /// 目标分辨率上限。
        max_resolution: Option<u32>,
    },
    /// 程序化生成整张地形（只算，不写场景）。
    Generate {
        /// 整张地形的尺寸 X（米）。
        size_x: f64,
        /// 整张地形的尺寸 Z（米）。
        size_z: f64,
        /// 随机种子。同种子必须同结果。
        seed: u64,
    },
    /// 读后端名（诊断「到底接没接真实现」）。
    Backend,
}

/// 诉求的应答。
#[derive(Debug, Clone, PartialEq)]
pub enum TerrainRsp {
    /// 采样结果。
    Sample(TerrainSampleDto),
    /// 块划分。
    Blocks(TerrainBlocksDto),
    /// 汇总统计。
    Stats(TerrainStatsDto),
    /// 重采样结果；`None` = 这块地形不存在或参数非法。
    Resample(Option<TerrainResampleDto>),
    /// 生成的块（行主序）；`None` = 尺寸非法或切块超上限。
    Generate(Option<Vec<TerrainPatch>>),
    /// 文本（后端名）。
    Text(String),
}

/// 脚本与宿主侧看到的地形能力面。
///
/// 实现方只需要写 `execute` 与 `as_any`；下面全是默认方法。
pub trait TerrainHost {
    /// 函数式适配器入口：`r3e-runtime` 用它把 `r3e-terrain` 的纯函数接进来。
    fn execute(&self, req: TerrainReq<'_>) -> TerrainRsp;

    /// 让装配层能从 trait 背后取回自己的实现体（`as_any().downcast_ref()`）。
    fn as_any(&self) -> &dyn Any;

    /// 世界点采样。
    fn sample(
        &self,
        patches: &[TerrainPatch],
        name: Option<&str>,
        x: f64,
        z: f64,
    ) -> TerrainSampleDto {
        match self.execute(TerrainReq::Sample {
            patches,
            name,
            x,
            z,
        }) {
            TerrainRsp::Sample(s) => s,
            _ => TerrainSampleDto::default(),
        }
    }

    /// 块划分。
    fn blocks(&self, patches: &[TerrainPatch], block_size: Option<f64>) -> TerrainBlocksDto {
        match self.execute(TerrainReq::Blocks {
            patches,
            block_size,
        }) {
            TerrainRsp::Blocks(b) => b,
            _ => TerrainBlocksDto::default(),
        }
    }

    /// 汇总统计。
    fn stats(&self, patches: &[TerrainPatch]) -> TerrainStatsDto {
        match self.execute(TerrainReq::Stats { patches }) {
            TerrainRsp::Stats(s) => s,
            _ => TerrainStatsDto::default(),
        }
    }

    /// 重采样（只算）。`None` = 这块地形不存在 / 参数非法 / 后端未接入。
    fn resample(
        &self,
        patch: &TerrainPatch,
        step: f64,
        max_resolution: Option<u32>,
    ) -> Option<TerrainResampleDto> {
        match self.execute(TerrainReq::Resample {
            patch,
            step,
            max_resolution,
        }) {
            TerrainRsp::Resample(r) => r,
            _ => None,
        }
    }

    /// 程序化生成（只算）。
    fn generate(&self, size_x: f64, size_z: f64, seed: u64) -> Option<Vec<TerrainPatch>> {
        match self.execute(TerrainReq::Generate {
            size_x,
            size_z,
            seed,
        }) {
            TerrainRsp::Generate(p) => p,
            _ => None,
        }
    }

    /// 后端名；未接真实实现时为 `"none"`。
    fn backend(&self) -> String {
        match self.execute(TerrainReq::Backend) {
            TerrainRsp::Text(t) => t,
            _ => "none".to_string(),
        }
    }
}

/// 空实现：采样回「没命中」、块划分回空表、统计回全零、生成/重采样回 `None`。
///
/// 用于单测 / 未装配地形后端的宿主。脚本侧据此打印提示，不 panic、不中断脚本。
#[derive(Debug, Default, Clone, Copy)]
pub struct NullTerrainHost;

impl NullTerrainHost {
    /// 构造。
    pub const fn new() -> Self {
        Self
    }
}

impl TerrainHost for NullTerrainHost {
    fn execute(&self, req: TerrainReq<'_>) -> TerrainRsp {
        match req {
            TerrainReq::Sample { .. } => TerrainRsp::Sample(TerrainSampleDto::default()),
            TerrainReq::Blocks { .. } => TerrainRsp::Blocks(TerrainBlocksDto::default()),
            TerrainReq::Stats { .. } => TerrainRsp::Stats(TerrainStatsDto::default()),
            TerrainReq::Resample { .. } => TerrainRsp::Resample(None),
            TerrainReq::Generate { .. } => TerrainRsp::Generate(None),
            TerrainReq::Backend => TerrainRsp::Text("none".to_string()),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 闭包即适配器：`F: Fn(TerrainReq) -> TerrainRsp` 直接就是一个地形后端。
///
/// 与物理 / 角色同一约定：测试里写一个闭包就是一套后端。
impl<F> TerrainHost for F
where
    F: for<'a> Fn(TerrainReq<'a>) -> TerrainRsp + 'static,
{
    fn execute(&self, req: TerrainReq<'_>) -> TerrainRsp {
        self(req)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch() -> TerrainPatch {
        TerrainPatch::flat("ground", [0.0, 0.0], [16.0, 16.0])
    }

    #[test]
    fn 空实现一律回空且不panic() {
        let host = NullTerrainHost::new();
        let patches = [patch()];
        let s = host.sample(&patches, None, 1.0, 1.0);
        assert!(!s.inside);
        assert_eq!(s.height, 0.0);
        assert_eq!(s.normal, [0.0, 1.0, 0.0]);
        assert_eq!(host.blocks(&patches, None).count, 0);
        assert_eq!(host.stats(&patches).patches, 0);
        assert!(host.resample(&patch(), 2.0, None).is_none());
        assert!(host.generate(100.0, 100.0, 7).is_none());
        assert_eq!(host.backend(), "none");
    }

    #[test]
    fn 空实现可以downcast() {
        let host = NullTerrainHost::new();
        assert!(host.as_any().downcast_ref::<NullTerrainHost>().is_some());
    }

    #[test]
    fn 闭包即适配器() {
        let host = |req: TerrainReq<'_>| match req {
            TerrainReq::Sample { .. } => TerrainRsp::Sample(TerrainSampleDto {
                name: "ground".to_string(),
                height: 3.5,
                ..TerrainSampleDto::default()
            }),
            TerrainReq::Stats { patches } => TerrainRsp::Stats(TerrainStatsDto {
                patches: patches.len(),
                ..TerrainStatsDto::default()
            }),
            TerrainReq::Backend => TerrainRsp::Text("stub".to_string()),
            _ => TerrainRsp::Generate(None),
        };
        let patches = [patch(), patch()];
        assert_eq!(host.sample(&patches, Some("ground"), 0.0, 0.0).height, 3.5);
        assert_eq!(host.stats(&patches).patches, 2);
        assert_eq!(host.backend(), "stub");
    }
}
