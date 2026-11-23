//! 脚本层「地形」七条命令的行为测试（M2-C06）。
//!
//! 锁六件事：
//! ① `terrain_create` 真的把块写进场景表，同名整体覆盖、空名 / 尺寸 / 分辨率越界一律拒；
//! ② `terrain_remove` 幂等（删过的再删回 `false`，不 panic）；
//! ③ `terrain_sample` / `blocks` / `stats` **委派给能力面**——用记录型 stub 证明
//!    脚本参数（块名 / 坐标 / 块边长）原样透传，且回的是 JSON；
//! ④ 指定了不存在的块名时回 `inside=false` 的空样本，不假装是平地；
//! ⑤ `terrain_gen` 要么全写、要么全不写：超表上限时场景零改动；
//! ⑥ `terrain_resample` 只在 `changed` 为真时落笔（平坦块不白占撤销栈）。
//!
//! 这里用 stub 而不是 `TerrainFacade`：`r3e-script` 不该为了测试把 `r3e-terrain`
//! 拖进依赖表（allow 红线），而「参数透传 + 落笔时机」这两件事本来就不依赖真算法。
//! 真算法的口径由 `r3e-terrain` 与 `r3e-runtime::terrain_host` 各自的单测看住。

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use r3e_core::host::{
    SilentAudioHost, TerrainBlocksDto, TerrainHost, TerrainReq, TerrainResampleDto, TerrainRsp,
    TerrainSampleDto, TerrainStatsDto,
};
use r3e_core::{InputState, Scene, TerrainPatch, TERRAIN_MAX_RESOLUTION, TERRAIN_MIN_SIZE};
use r3e_doc::Document;
use r3e_physics::{Physics, PhysicsFacade};
use r3e_script::ScriptHost;
use serde_json::Value;

/// 记录型能力面：把每次诉求写成一行痕迹，回包内容由测试预设。
#[derive(Default)]
struct SpyTerrain {
    calls: RefCell<Vec<String>>,
    gen: RefCell<Option<Vec<TerrainPatch>>>,
    resample: RefCell<Option<TerrainResampleDto>>,
}

impl SpyTerrain {
    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

impl TerrainHost for SpyTerrain {
    fn execute(&self, req: TerrainReq<'_>) -> TerrainRsp {
        match req {
            TerrainReq::Sample {
                patches,
                name,
                x,
                z,
            } => {
                self.calls.borrow_mut().push(format!(
                    "sample name={:?} x={x} z={z} patches={}",
                    name,
                    patches.len()
                ));
                TerrainRsp::Sample(TerrainSampleDto {
                    name: name.unwrap_or("stub").to_string(),
                    height: 2.5,
                    normal: [0.0, 1.0, 0.0],
                    inside: true,
                    weights: [0.25, 0.5, 0.25, 0.0],
                })
            }
            TerrainReq::Blocks {
                patches,
                block_size,
            } => {
                self.calls.borrow_mut().push(format!(
                    "blocks block_size={block_size:?} patches={}",
                    patches.len()
                ));
                TerrainRsp::Blocks(TerrainBlocksDto {
                    rect: [0.0, 0.0, 10.0, 10.0],
                    block_size: block_size.unwrap_or(50.0),
                    dims: [1, 1],
                    count: 1,
                    covered: patches.len() as u32,
                    blocks: Vec::new(),
                })
            }
            TerrainReq::Stats { patches } => {
                self.calls
                    .borrow_mut()
                    .push(format!("stats patches={}", patches.len()));
                TerrainRsp::Stats(TerrainStatsDto {
                    patches: patches.len(),
                    ..Default::default()
                })
            }
            TerrainReq::Resample {
                patch,
                step,
                max_resolution,
            } => {
                self.calls.borrow_mut().push(format!(
                    "resample name={} step={step} max={max_resolution:?}",
                    patch.name
                ));
                TerrainRsp::Resample(self.resample.borrow().clone())
            }
            TerrainReq::Generate {
                size_x,
                size_z,
                seed,
            } => {
                self.calls
                    .borrow_mut()
                    .push(format!("generate size=({size_x},{size_z}) seed={seed}"));
                TerrainRsp::Generate(self.gen.borrow().clone())
            }
            TerrainReq::Backend => TerrainRsp::Text("spy".to_string()),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct Fixture {
    scene: Rc<RefCell<Document>>,
    spy: Rc<SpyTerrain>,
    host: ScriptHost,
}

fn fixture() -> Fixture {
    let scene = Rc::new(RefCell::new(Document::new(Scene::new())));
    let physics = Rc::new(RefCell::new(Physics::new()));
    let input = Rc::new(RefCell::new(InputState::default()));
    let spy = Rc::new(SpyTerrain::default());
    let host = ScriptHost::new_with_full(
        scene.clone(),
        PhysicsFacade::wrap(physics),
        input,
        Rc::new(SilentAudioHost::new("测试静默音频")),
        Rc::new(r3e_core::host::NullCharacterHost::new()),
        spy.clone(),
    );
    Fixture { scene, spy, host }
}

impl Fixture {
    fn eval(&mut self, src: &str) -> bool {
        self.host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"))
            .as_bool()
            .unwrap_or_else(|_| panic!("{src} 应返回布尔"))
    }

    fn eval_json(&mut self, src: &str) -> Value {
        let text = self
            .host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"))
            .into_string()
            .unwrap_or_else(|_| panic!("{src} 应返回字符串"));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{src} 应回合法 JSON: {e}\n{text}"))
    }

    fn patches(&self) -> Vec<TerrainPatch> {
        self.scene.borrow().as_scene().terrain.clone()
    }
}

#[test]
fn create_writes_patch_and_overwrites_same_name() {
    let mut f = fixture();
    assert!(f.eval("terrain_create(\"ground\", 0.0, 0.0, 100.0, 100.0, 16)"));
    let patches = f.patches();
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0].name, "ground");
    assert_eq!(patches[0].resolution, [16, 16]);

    // 同名整体覆盖：不是追加。
    assert!(f.eval("terrain_create(\"ground\", 10.0, 10.0, 50.0, 50.0, 8)"));
    let patches = f.patches();
    assert_eq!(patches.len(), 1, "同名必须覆盖，不得追加");
    assert_eq!(patches[0].resolution, [8, 8]);
    assert_eq!(patches[0].origin, [10.0, 10.0]);
}

#[test]
fn create_rejects_bad_arguments() {
    let mut f = fixture();
    assert!(
        !f.eval("terrain_create(\"\", 0.0, 0.0, 100.0, 100.0, 16)"),
        "空名应拒"
    );
    assert!(
        !f.eval(&format!(
            "terrain_create(\"g\", 0.0, 0.0, {}, 100.0, 16)",
            TERRAIN_MIN_SIZE / 2.0
        )),
        "尺寸小于下限应拒"
    );
    assert!(
        !f.eval("terrain_create(\"g\", 0.0, 0.0, 100.0, 100.0, 0)"),
        "分辨率 0 应拒"
    );
    assert!(
        !f.eval(&format!(
            "terrain_create(\"g\", 0.0, 0.0, 100.0, 100.0, {})",
            TERRAIN_MAX_RESOLUTION + 1
        )),
        "分辨率越上限应拒（收敛会悄悄变形，不如直接拒）"
    );
    assert!(
        !f.eval("terrain_create(\"g\", 0.0/0.0, 0.0, 100.0, 100.0, 16)"),
        "非有限坐标应拒"
    );
    assert!(f.patches().is_empty(), "被拒的调用不得留下半块地形");
}

#[test]
fn remove_is_idempotent() {
    let mut f = fixture();
    f.eval("terrain_create(\"ground\", 0.0, 0.0, 100.0, 100.0, 16)");
    assert!(f.eval("terrain_remove(\"ground\")"));
    assert!(f.patches().is_empty());
    assert!(
        !f.eval("terrain_remove(\"ground\")"),
        "删过的再删应回 false"
    );
    assert!(!f.eval("terrain_remove(\"\")"), "空名应拒");
}

#[test]
fn sample_passes_arguments_through_and_returns_json() {
    let mut f = fixture();
    f.eval("terrain_create(\"ground\", 0.0, 0.0, 100.0, 100.0, 16)");
    let v = f.eval_json("terrain_sample(\"ground\", 12.5, -3.0)");
    assert_eq!(v["name"], "ground");
    assert_eq!(v["height"], 2.5);
    assert_eq!(v["inside"], true);
    assert_eq!(v["weights"][1], 0.5);

    let calls = f.spy.calls();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].contains("name=Some(\"ground\")") && calls[0].contains("x=12.5"),
        "采样参数应原样透传给能力面，实际: {}",
        calls[0]
    );
    assert!(
        calls[0].contains("patches=1"),
        "应把场景地形块整体交给能力面"
    );
}

#[test]
fn sample_with_empty_name_asks_for_any_patch() {
    let mut f = fixture();
    f.eval("terrain_create(\"ground\", 0.0, 0.0, 100.0, 100.0, 16)");
    f.eval_json("terrain_sample(\"\", 1.0, 1.0)");
    let calls = f.spy.calls();
    assert!(
        calls[0].contains("name=None"),
        "空块名应翻译成「跨块取最高层」(None)，实际: {}",
        calls[0]
    );
}

#[test]
fn sample_unknown_patch_reports_empty_sample() {
    let mut f = fixture();
    let v = f.eval_json("terrain_sample(\"nope\", 0.0, 0.0)");
    assert_eq!(v["inside"], false, "不存在的块不得假装是平地");
    assert_eq!(v["height"], 0.0);
    assert_eq!(v["name"], "");
    assert!(f.spy.calls().is_empty(), "块名查不到时不该白跑一趟能力面");
}

#[test]
fn blocks_and_stats_delegate_and_return_json() {
    let mut f = fixture();
    f.eval("terrain_create(\"a\", 0.0, 0.0, 10.0, 10.0, 8)");
    f.eval("terrain_create(\"b\", 10.0, 0.0, 10.0, 10.0, 8)");

    let b = f.eval_json("terrain_blocks()");
    assert_eq!(b["covered"], 2);
    assert_eq!(b["count"], 1);

    let s = f.eval_json("terrain_stats()");
    assert_eq!(s["patches"], 2);

    let calls = f.spy.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].starts_with("blocks"), "实际: {}", calls[0]);
    assert!(calls[1].starts_with("stats"), "实际: {}", calls[1]);
}

#[test]
fn gen_writes_all_or_nothing() {
    let mut f = fixture();
    let mut a = TerrainPatch::flat("gen_0_0", [0.0, 0.0], [50.0, 50.0]);
    a.resolution = [8, 8];
    let mut b = TerrainPatch::flat("gen_1_0", [50.0, 0.0], [50.0, 50.0]);
    b.resolution = [8, 8];
    *f.spy.gen.borrow_mut() = Some(vec![a, b]);

    let v = f.eval_json("terrain_gen(100, 50, 42)");
    assert_eq!(v["created"], 2);
    assert_eq!(v["names"].as_array().map(|n| n.len()), Some(2));
    assert!(!v["state_hash"].as_str().unwrap_or("").is_empty());
    assert_eq!(f.patches().len(), 2);
    let calls = f.spy.calls();
    assert!(calls[0].contains("seed=42"), "实际: {}", calls[0]);

    // 尺寸非法：连能力面都不该跑，场景零改动。
    let v = f.eval_json("terrain_gen(0, 50, 42)");
    assert_eq!(v["created"], 0);
    assert_eq!(f.patches().len(), 2);
}

#[test]
fn gen_reports_zero_when_backend_declines() {
    let mut f = fixture();
    // stub 未预设 = 能力面回 None（尺寸超切块上限之类的拒法）。
    let v = f.eval_json("terrain_gen(100, 50, 7)");
    assert_eq!(v["created"], 0);
    assert_eq!(v["names"].as_array().map(|n| n.len()), Some(0));
    assert!(f.patches().is_empty());
}

#[test]
fn resample_only_writes_when_changed() {
    let mut f = fixture();
    f.eval("terrain_create(\"ground\", 0.0, 0.0, 100.0, 100.0, 16)");

    // 平坦块：能力面回 changed=false。
    let mut same = TerrainPatch::flat("ground", [0.0, 0.0], [100.0, 100.0]);
    same.resolution = [16, 16];
    *f.spy.resample.borrow_mut() = Some(TerrainResampleDto {
        from: [16, 16],
        to: [8, 8],
        samples: 64,
        max_error: 0.0,
        patch: same,
        changed: false,
    });
    assert!(f.eval("terrain_resample(\"ground\", 2.0, 0)"));
    assert_eq!(
        f.patches()[0].resolution,
        [16, 16],
        "changed=false 时不该改场景"
    );

    // 真改动：落笔。
    let mut shrunk = TerrainPatch::flat("ground", [0.0, 0.0], [100.0, 100.0]);
    shrunk.resolution = [8, 8];
    *f.spy.resample.borrow_mut() = Some(TerrainResampleDto {
        from: [16, 16],
        to: [8, 8],
        samples: 64,
        max_error: 0.01,
        patch: shrunk,
        changed: true,
    });
    assert!(f.eval("terrain_resample(\"ground\", 2.0, 0)"));
    assert_eq!(f.patches()[0].resolution, [8, 8]);

    let calls = f.spy.calls();
    assert!(
        calls[0].contains("max=None"),
        "0 应翻译成「用全局上限」，实际: {}",
        calls[0]
    );
}

#[test]
fn resample_rejects_unknown_patch_and_bad_cap() {
    let mut f = fixture();
    assert!(!f.eval("terrain_resample(\"nope\", 2.0, 0)"));
    f.eval("terrain_create(\"ground\", 0.0, 0.0, 100.0, 100.0, 16)");
    assert!(
        !f.eval("terrain_resample(\"ground\", 2.0, 1)"),
        "上限 1 应拒"
    );
    assert!(
        !f.eval(&format!(
            "terrain_resample(\"ground\", 2.0, {})",
            TERRAIN_MAX_RESOLUTION + 1
        )),
        "上限越界应拒"
    );
    assert!(f.spy.calls().is_empty(), "被拒的调用不该碰能力面");
}

#[test]
fn null_backend_keeps_create_and_remove_working() {
    // 没接地形能力面时：两条只碰场景表的命令照常可用，其余回空——不 panic。
    let scene = Rc::new(RefCell::new(Document::new(Scene::new())));
    let physics = Rc::new(RefCell::new(Physics::new()));
    let input = Rc::new(RefCell::new(InputState::default()));
    let mut host = ScriptHost::new_with_full(
        scene.clone(),
        PhysicsFacade::wrap(physics),
        input,
        Rc::new(SilentAudioHost::new("测试静默音频")),
        Rc::new(r3e_core::host::NullCharacterHost::new()),
        Rc::new(r3e_core::host::NullTerrainHost::new()),
    );
    let ok = host
        .eval("terrain_create(\"g\", 0.0, 0.0, 100.0, 100.0, 16)")
        .expect("应能执行")
        .as_bool()
        .expect("应返回布尔");
    assert!(ok);
    assert_eq!(scene.borrow().as_scene().terrain.len(), 1);
    let sample = host
        .eval("terrain_sample(\"g\", 1.0, 1.0)")
        .expect("应能执行")
        .into_string()
        .expect("应返回字符串");
    assert!(sample.contains("\"inside\":false"), "实际: {sample}");
}
