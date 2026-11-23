//! 地形能力面的引擎侧实现（M2-C06，CD-2 的落点）。
//!
//! `r3e-core` 只声明 [`TerrainHost`]，真实现放这里，因为只有本 crate 同时依赖
//! `r3e-terrain` 与 `r3e-doc`：脚本宿主（`r3e-script`）与编辑器宿主都不能直接
//! 调 `r3e-terrain`（`xtask/dep_rules.json` 的 allow 红线）。
//!
//! ## 实现策略：影子文档
//!
//! 四条只读命令 + 两条「只算不写」命令的计算逻辑已经在
//! [`crate::terrain_ops`]（RPC 侧 `terrain.*` 的同一份实现）。这里**不重写算法**：
//! 把调用方递进来的地形块拼成一份临时 `Document`（影子文档），交给
//! `terrain_ops` 现算，再把结果翻成 `core` 的 DTO。
//!
//! 好处是口径**字节级**一致——`terrain_sample` 与 RPC `terrain.sample` 不可能分叉；
//! 代价是每次调用要克隆一份地形块表（32 块 × 128² 约几 MB）。脚本侧这七条命令
//! 不是逐帧热路径（`terrain_stats` 例外，见下），克隆换正确性值得。
//!
//! 影子文档的写入**一律不回流**：`resample` / `gen` 在影子上算出来的块，
//! 由脚本侧调 `Scene::upsert_terrain_patch` 真正落笔（脚本侧的写入不进编辑器撤销栈，
//! `cmd_spec` 里这四条 `reversible` 都是 `false`；`state_hash` 照常覆盖，因为它算的是 `Scene`）。
//!
//! ## 为什么 `stats` 不用影子文档
//!
//! `terrain_stats` 可能被脚本在 `on_update` 里逐帧调；克隆整张地形块表只为数三角数
//! 太贵。这里直接调 `terrain_ops::stats`（它自己会 clone 一份 `Scene`，
//! 但仍然只是一次浅拷贝级别的开销）—— 与 RPC 侧同一个函数，口径不变。
//!
//! 实际上 `terrain_ops::stats(doc)` 需要 `&Document`，所以依旧要造影子文档。
//! 于是折中：**一次调用一个影子文档，六条命令共用同一条构造路径**
//! （[`shadow`]），不做额外优化——先把正确性钉死，性能留到有实测数据再谈。

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use r3e_core::host::{
    TerrainBlockDto, TerrainBlocksDto, TerrainHost, TerrainReq, TerrainResampleDto, TerrainRsp,
    TerrainSampleDto, TerrainStatsDto,
};
use r3e_core::{Scene, TerrainPatch};
use r3e_doc::Document;

use crate::terrain_ops;

/// 真地形能力面的门面：把诉求转给 [`crate::terrain_ops`]。
///
/// 无状态、可克隆、`Send + Sync` 不需要：脚本宿主是单线程 `Rc` 上下文。
#[derive(Debug, Default, Clone, Copy)]
pub struct TerrainFacade;

impl TerrainFacade {
    /// 构造。
    pub const fn new() -> Self {
        Self
    }
}

/// 装配入口：拿**真实**物理学 / 音频 / 地形能力面构造脚本宿主（M2-C06）。
///
/// 只有本 crate 能这么干：`r3e-script` 不认识 `Physics` / `Audio` / [`TerrainFacade`]，
/// 而 `app.rs` 在 `size_debt` 名单里（新增 >100 行要提前 G02），所以把「怎么拼」
/// 收进这里，`app.rs` 只留一行调用——装配知识放装配层，编辑器 / 回归夹具要换后端时
/// 也只改这一处。
///
/// 角色控制器仍是空实现：B01 只把脚本侧能力面铺好，真装配留到 D 阶段
/// （与 `M2_B_PLAN` 的收口口径一致，不在本卡偷偷扩围）。
pub fn script_host(
    doc: Rc<RefCell<Document>>,
    physics: Rc<RefCell<r3e_physics::Physics>>,
    input: Rc<RefCell<r3e_core::InputState>>,
    audio: Rc<RefCell<r3e_audio::Audio>>,
) -> r3e_script::ScriptHost {
    r3e_script::ScriptHost::new_with_full(
        doc,
        r3e_physics::PhysicsFacade::wrap(physics),
        input,
        r3e_audio::AudioFacade::wrap(audio),
        Rc::new(r3e_core::host::NullCharacterHost::new()),
        Rc::new(TerrainFacade::new()),
    )
}

/// 把一串地形块拼成一份临时文档（影子文档）。
///
/// 只用来跑现算逻辑，调用方拿到结果后立刻丢弃；写进去的 `CommandKind` 一律不回流。
fn shadow(patches: &[TerrainPatch]) -> Document {
    let mut scene = Scene::default();
    scene.terrain = patches.to_vec();
    Document::new(scene)
}

impl TerrainHost for TerrainFacade {
    fn execute(&self, req: TerrainReq<'_>) -> TerrainRsp {
        match req {
            TerrainReq::Sample {
                patches,
                name,
                x,
                z,
            } => {
                let doc = shadow(patches);
                // 坐标非有限时 `terrain_ops::sample` 回 `Invalid`：这里翻成「没命中」，
                // 脚本拿到 `inside == false` 就知道这个点没有地形，不必区分是哪种没有。
                let out = match terrain_ops::sample(&doc, name, x as f32, z as f32) {
                    Ok(o) => o,
                    Err(_) => return TerrainRsp::Sample(TerrainSampleDto::default()),
                };
                TerrainRsp::Sample(TerrainSampleDto {
                    name: out.name,
                    height: out.height as f64,
                    normal: [
                        out.normal[0] as f64,
                        out.normal[1] as f64,
                        out.normal[2] as f64,
                    ],
                    inside: out.inside,
                    weights: [
                        out.weights[0] as f64,
                        out.weights[1] as f64,
                        out.weights[2] as f64,
                        out.weights[3] as f64,
                    ],
                })
            }
            TerrainReq::Blocks {
                patches,
                block_size,
            } => {
                let doc = shadow(patches);
                let out = match terrain_ops::blocks(&doc, block_size.map(|v| v as f32)) {
                    Ok(o) => o,
                    Err(_) => return TerrainRsp::Blocks(TerrainBlocksDto::default()),
                };
                TerrainRsp::Blocks(TerrainBlocksDto {
                    rect: [
                        out.rect[0] as f64,
                        out.rect[1] as f64,
                        out.rect[2] as f64,
                        out.rect[3] as f64,
                    ],
                    block_size: out.block_size as f64,
                    dims: out.dims,
                    count: out.count,
                    covered: out.covered,
                    blocks: out
                        .blocks
                        .into_iter()
                        .map(|b| TerrainBlockDto {
                            index: b.index,
                            rect: [
                                b.rect[0] as f64,
                                b.rect[1] as f64,
                                b.rect[2] as f64,
                                b.rect[3] as f64,
                            ],
                            patches: b.patches,
                        })
                        .collect(),
                })
            }
            TerrainReq::Stats { patches } => {
                let doc = shadow(patches);
                let out = match terrain_ops::stats(&doc) {
                    Ok(o) => o,
                    Err(_) => return TerrainRsp::Stats(TerrainStatsDto::default()),
                };
                TerrainRsp::Stats(TerrainStatsDto {
                    patches: out.patches,
                    samples: out.samples,
                    blocks: out.blocks,
                    covered: out.covered,
                    triangles: out.triangles,
                    vertices: out.vertices,
                    lod_step: out.lod.step() as f64,
                    block_size: out.block_size as f64,
                })
            }
            TerrainReq::Resample {
                patch,
                step,
                max_resolution,
            } => {
                // 影子文档里只放这一块：`terrain_ops::resample` 按名字找块，
                // 单块进、单块出，不会碰到别的块。
                let mut doc = shadow(std::slice::from_ref(patch));
                let out =
                    match terrain_ops::resample(&mut doc, &patch.name, step as f32, max_resolution)
                    {
                        Ok(o) => o,
                        Err(_) => return TerrainRsp::Resample(None),
                    };
                // 平坦块不落笔：影子文档里仍是原块，`changed` 也就自然是 false。
                let stored = doc
                    .as_scene()
                    .terrain
                    .iter()
                    .find(|p| p.name == patch.name)
                    .cloned()
                    .unwrap_or_else(|| patch.clone());
                let changed = stored != *patch;
                TerrainRsp::Resample(Some(TerrainResampleDto {
                    from: out.from,
                    to: out.to,
                    samples: out.samples,
                    max_error: out.max_error as f64,
                    patch: stored,
                    changed,
                }))
            }
            TerrainReq::Generate {
                size_x,
                size_z,
                seed,
            } => {
                // 生成时必须从空场景起算：容量预检由脚本侧对**真场景**做，
                // 影子文档只管「这组参数能生成哪些块」。
                let mut doc = Document::new(Scene::default());
                match terrain_ops::generate_into(&mut doc, size_x as f32, size_z as f32, seed) {
                    Ok(_) => TerrainRsp::Generate(Some(doc.as_scene().terrain.clone())),
                    Err(_) => TerrainRsp::Generate(None),
                }
            }
            TerrainReq::Backend => TerrainRsp::Text("r3e-terrain".to_string()),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::host::NullTerrainHost;

    fn patch(name: &str) -> TerrainPatch {
        TerrainPatch::flat(name, [0.0, 0.0], [16.0, 16.0])
    }

    #[test]
    fn 后端名不是none() {
        assert_eq!(TerrainFacade::new().backend(), "r3e-terrain");
    }

    #[test]
    fn 平坦地形采样命中且权重归一() {
        let host = TerrainFacade::new();
        let patches = [patch("ground")];
        let s = host.sample(&patches, Some("ground"), 8.0, 8.0);
        assert!(s.inside);
        assert_eq!(s.name, "ground");
        assert!((s.height - 0.0).abs() < 1e-6);
        let sum: f64 = s.weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
    }

    #[test]
    fn 块外采样回没命中而不是零高度地形() {
        let host = TerrainFacade::new();
        let patches = [patch("ground")];
        let s = host.sample(&patches, Some("ground"), 100.0, 100.0);
        assert!(!s.inside);
        assert_eq!(s.normal, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn 空地形不报错且统计全零() {
        let host = TerrainFacade::new();
        assert_eq!(host.blocks(&[], None).count, 0);
        let stats = host.stats(&[]);
        assert_eq!(stats.patches, 0);
        assert_eq!(stats.triangles, 0);
    }

    #[test]
    fn 块划分覆盖数不超过总数() {
        let host = TerrainFacade::new();
        let patches = [patch("a")];
        let b = host.blocks(&patches, Some(8.0));
        assert_eq!(b.count, 4);
        assert_eq!(b.covered, 4);
        assert!(b
            .blocks
            .iter()
            .all(|blk| blk.patches == vec!["a".to_string()]));
    }

    #[test]
    fn 生成同种子结果一致且块名可复用() {
        let host = TerrainFacade::new();
        let a = host.generate(64.0, 64.0, 42).expect("应能生成");
        let b = host.generate(64.0, 64.0, 42).expect("应能生成");
        assert_eq!(a, b);
        assert!(!a.is_empty());
        let c = host.generate(64.0, 64.0, 43).expect("应能生成");
        assert_ne!(a, c);
    }

    #[test]
    fn 尺寸非法时生成回none() {
        let host = TerrainFacade::new();
        assert!(host.generate(0.0, 64.0, 1).is_none());
    }

    #[test]
    fn 平坦块重采样不算改动() {
        let host = TerrainFacade::new();
        let out = host.resample(&patch("ground"), 2.0, None).expect("应能算");
        assert!(!out.changed, "平坦块没有可抽稀的网格，不该算改动");
    }

    #[test]
    fn 空实现与真实现可区分() {
        assert_eq!(NullTerrainHost::new().backend(), "none");
        assert!(TerrainFacade::new()
            .as_any()
            .downcast_ref::<TerrainFacade>()
            .is_some());
    }
}
