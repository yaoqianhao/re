//! 脚本层 1D / 2D / 3D 混合空间绑定的行为测试（扩展四十五 / 四十九）。
//!
//! `r3e-core` 的单测保证了几何算法本身对不对，但「脚本调完之后骨骼到底怎么动」只有真跑
//! Rhai 才能确认。这里覆盖六件事：
//! ① 挂空间之前层仍走手写权重（零变化），挂空间之后权重由参数算出来；
//! ② 一维参数推到端点 / 中点时的姿态落点；
//! ③ 二维四角样本在中心各占一半、框外外插钳制到最近边；
//! ④ 回显字符串（`anim_layer_blend`）能 grep 到维度、样本数、参数值与权重；
//! ⑤ 摘掉空间后回到手写权重，实例/层不存在时返回中性值；
//! ⑥ 三维空间（四面体剖分）的顶点 / 棱上落点，以及二维入口在三维空间上等价于 z=0。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use r3e_core::anim::{AnimBinding, AnimInstance};
use r3e_core::{AnimClip, AnimKey, AnimTrack, ObjectDesc, Scene, Vec3};
use r3e_physics::Physics;
use r3e_script::ScriptHost;
use rhai::Dynamic;

const INST: &str = "demo/walk";

struct Fixture {
    scene: Rc<RefCell<Scene>>,
    host: ScriptHost,
}

fn line_track(index: u32, path: &str, end: Vec3) -> AnimTrack {
    AnimTrack {
        node_path: path.to_string(),
        node_index: index,
        keys: vec![
            AnimKey {
                time: 0.0,
                position: Vec3::ZERO,
                rotation: r3e_core::Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            AnimKey {
                time: 1.0,
                position: end,
                rotation: r3e_core::Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        ],
    }
}

/// arm（下标 1）+ leg（下标 2）：walk 把两者推到 x=10，atk 把 arm 推到 x=100，idle 恒 0。
fn fixture() -> Fixture {
    let mut scene = Scene::new();
    let arm = scene
        .spawn(ObjectDesc {
            name: "arm".to_string(),
            ..Default::default()
        })
        .unwrap();
    let leg = scene
        .spawn(ObjectDesc {
            name: "leg".to_string(),
            ..Default::default()
        })
        .unwrap();

    let walk = AnimClip {
        name: "walk".to_string(),
        duration: 1.0,
        tracks: vec![
            line_track(1, "root/arm", Vec3::new(10.0, 0.0, 0.0)),
            line_track(2, "root/leg", Vec3::new(10.0, 0.0, 0.0)),
        ],
    };
    let atk = AnimClip {
        name: "atk".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0))],
    };
    let idle = AnimClip {
        name: "idle".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::ZERO)],
    };

    let mut inst = AnimInstance::new(INST, Arc::new(walk));
    inst.bindings = vec![
        AnimBinding {
            node: arm,
            node_index: 1,
            path: "root/arm".to_string(),
            track: 0,
        },
        AnimBinding {
            node: leg,
            node_index: 2,
            path: "root/leg".to_string(),
            track: 1,
        },
    ];
    scene.add_anim_instance(inst);
    scene.register_anim_clip("demo/atk", Arc::new(atk));
    scene.register_anim_clip("demo/idle", Arc::new(idle));

    let scene = Rc::new(RefCell::new(scene));
    let physics = Rc::new(RefCell::new(Physics::new()));
    let input = Rc::new(RefCell::new(r3e_core::InputState::default()));
    let host = ScriptHost::new(scene.clone(), physics, input);
    Fixture { scene, host }
}

impl Fixture {
    fn eval(&mut self, src: &str) -> Dynamic {
        self.host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"))
    }

    fn bool_of(&mut self, src: &str) -> bool {
        self.eval(src)
            .as_bool()
            .unwrap_or_else(|_| panic!("`{src}` 应返回 bool"))
    }

    fn string_of(&mut self, src: &str) -> String {
        self.eval(src)
            .into_string()
            .unwrap_or_else(|_| panic!("`{src}` 应返回字符串"))
    }

    fn advance(&self, dt: f32) {
        self.scene.borrow_mut().update_animations(dt);
    }

    fn pos_x(&self, node: &str) -> f32 {
        let scene = self.scene.borrow();
        let id = scene.find_id(node).expect("节点应存在");
        scene.get(id).unwrap().transform.position.x
    }
}

#[test]
fn script_1d_blend_space_drives_layer_weights() {
    let mut f = fixture();
    // 层里两路片段：idle 权重 1、atk 权重 0（手写值）
    assert!(f.bool_of(r#"add_anim_layer("demo/walk", "move", "idle", 1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_clip("demo/walk", "move", "atk", 0.0)"#));

    // ① 没挂空间：手写权重说了算，arm 恒 0（覆盖型层整副骨架生效）
    f.advance(0.5);
    let x = f.pos_x("arm");
    assert!(x.abs() < 1e-4, "未挂空间时应走手写权重，实际 {x}");

    // ② 挂一维空间 + 两个样本
    assert!(f.bool_of(r#"set_anim_layer_blend_1d("demo/walk", "move", "speed")"#));
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "idle", 0.0, 0.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "atk", 1.0, 0.0)"#));

    // ③ 参数 0.5：两路各半；t=0.5 时 idle=0、atk=50 → 25
    assert!(f.bool_of(r#"set_anim_layer_blend_value("demo/walk", "move", 0.5, 0.0)"#));
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((x - 25.0).abs() < 1e-3, "参数 0.5 应混合到 25，实际 {x}");

    let line = f.string_of(r#"anim_layer_blend("demo/walk", "move")"#);
    assert!(
        line.starts_with("1d(speed) n=2 drop=0 v=0.50 w="),
        "回显前缀不对: {line}"
    );
    assert!(
        line.contains("idle=0.50") && line.contains("atk=0.50"),
        "回显权重不对: {line}"
    );

    // ④ 参数推到 1：全 atk → 50
    assert!(f.bool_of(r#"set_anim_layer_blend_value("demo/walk", "move", 1.0, 0.0)"#));
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((x - 50.0).abs() < 1e-3, "参数 1.0 应取 atk，实际 {x}");
}

#[test]
fn script_2d_blend_space_quad_center_and_clamp() {
    let mut f = fixture();
    assert!(f.bool_of(r#"add_anim_layer("demo/walk", "move", "idle", 1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_clip("demo/walk", "move", "atk", 0.0)"#));
    assert!(f.bool_of(r#"set_anim_layer_blend_2d("demo/walk", "move", "move.x", "move.y")"#));

    // 四角：idle 占左列、atk 占右列
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "idle", -1.0, -1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "atk", 1.0, -1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "idle", -1.0, 1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "atk", 1.0, 1.0)"#));

    // 中心：两路各半 → 25（t=0.5 时 idle=0、atk=50）
    assert!(f.bool_of(r#"set_anim_layer_blend_value("demo/walk", "move", 0.0, 0.0)"#));
    f.advance(0.5);
    let x = f.pos_x("arm");
    assert!((x - 25.0).abs() < 1e-3, "中心应各半，实际 {x}");

    let line = f.string_of(r#"anim_layer_blend("demo/walk", "move")"#);
    assert!(
        line.starts_with("2d(move.x,move.y) n=4 tri=2 drop=0 v=(0.00,0.00) w="),
        "回显前缀不对: {line}"
    );

    // 推到框外右上方：外插钳制到最近边 → 全 atk
    assert!(f.bool_of(r#"set_anim_layer_blend_value("demo/walk", "move", 9.0, 9.0)"#));
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((x - 50.0).abs() < 1e-3, "框外应钳制到 atk，实际 {x}");

    // 样本坐标重复要被拒（脚本拿到 false，不会静默多出一路）
    assert!(!f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "idle", 1.0, 1.0)"#));
}

#[test]
fn script_blend_space_cleared_falls_back_to_manual_weights() {
    let mut f = fixture();
    assert!(f.bool_of(r#"add_anim_layer("demo/walk", "move", "idle", 1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_clip("demo/walk", "move", "atk", 0.0)"#));
    assert!(f.bool_of(r#"set_anim_layer_blend_1d("demo/walk", "move", "speed")"#));
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "idle", 0.0, 0.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "atk", 1.0, 0.0)"#));
    assert!(f.bool_of(r#"set_anim_layer_blend_value("demo/walk", "move", 1.0, 0.0)"#));
    f.advance(0.5);
    assert!((f.pos_x("arm") - 50.0).abs() < 1e-3, "空间应把权重推到 atk");

    // 摘掉空间：权重冻结在最后一次空间算出的结果上（不跳变），回显变空串
    assert!(f.bool_of(r#"clear_anim_layer_blend("demo/walk", "move")"#));
    f.advance(0.0);
    assert!(
        (f.pos_x("arm") - 50.0).abs() < 1e-3,
        "摘掉空间不应让姿态跳变"
    );
    assert_eq!(f.string_of(r#"anim_layer_blend("demo/walk", "move")"#), "");

    // 冻结之后手写权重重新接管：两路权重都写回去（idle 1 / atk 0）→ arm 回 0
    assert!(f.bool_of(r#"set_anim_layer_clip_weight("demo/walk", "move", "idle", 1.0)"#));
    assert!(f.bool_of(r#"set_anim_layer_clip_weight("demo/walk", "move", "atk", 0.0)"#));
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!(x.abs() < 1e-4, "手写权重应重新接管，实际 {x}");
}

#[test]
fn script_3d_blend_space_drives_layer_weights() {
    let mut f = fixture();
    assert!(f.bool_of(r#"add_anim_layer("demo/walk", "move", "idle", 1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_clip("demo/walk", "move", "atk", 0.0)"#));
    // 挂之前仍然走手写权重
    f.advance(0.5);
    let x = f.pos_x("arm");
    assert!(x.abs() < 1e-4, "未挂三维空间时应走手写权重，实际 {x}");

    assert!(
        f.bool_of(r#"set_anim_layer_blend_3d("demo/walk", "move", "move.x", "move.y", "speed")"#)
    );
    // 四个不共面样本 → 一个四面体：idle 占原点角，atk 占另外三个角
    assert!(f.bool_of(
        r#"add_anim_layer_blend_sample_3d("demo/walk", "move", "idle", -1.0, -1.0, -1.0)"#
    ));
    assert!(
        f.bool_of(r#"add_anim_layer_blend_sample_3d("demo/walk", "move", "atk", 1.0, -1.0, -1.0)"#)
    );
    assert!(
        f.bool_of(r#"add_anim_layer_blend_sample_3d("demo/walk", "move", "atk", 1.0, 1.0, -1.0)"#)
    );
    assert!(
        f.bool_of(r#"add_anim_layer_blend_sample_3d("demo/walk", "move", "atk", 1.0, 1.0, 1.0)"#)
    );
    // 同一位置再来一份要被拒（否则等于偷偷加一路平均姿态）
    assert!(!f.bool_of(
        r#"add_anim_layer_blend_sample_3d("demo/walk", "move", "idle", -1.0, -1.0, -1.0)"#
    ));

    // 棱中点 idle-atk：各半 → 25（t=0.5 时 idle=0、atk=50）
    assert!(f.bool_of(r#"set_anim_layer_blend_value_3d("demo/walk", "move", 0.0, -1.0, -1.0)"#));
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((x - 25.0).abs() < 1e-3, "棱中点应各半，实际 {x}");

    let line = f.string_of(r#"anim_layer_blend("demo/walk", "move")"#);
    assert!(
        line.starts_with("3d(move.x,move.y,speed) n=4 tet=1 drop=1 v=(0.00,-1.00,-1.00) w="),
        "回显前缀不对: {line}"
    );
    assert!(
        line.contains("idle=0.50") && line.contains("atk=0.50"),
        "回显权重不对: {line}"
    );

    // 顶点：全 atk → 50
    assert!(f.bool_of(r#"set_anim_layer_blend_value_3d("demo/walk", "move", 1.0, -1.0, -1.0)"#));
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((x - 50.0).abs() < 1e-3, "顶点应取 atk，实际 {x}");

    // 二维入口在三维空间上等价于 z=0：四面体内部，权重仍归一
    assert!(f.bool_of(r#"set_anim_layer_blend_value("demo/walk", "move", 1.0, 1.0)"#));
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((0.0..=50.0).contains(&x), "二维入口不应越界，实际 {x}");

    // 三维入口的缺失目标仍是 false，不 panic
    assert!(!f.bool_of(r#"set_anim_layer_blend_3d("nobody", "nope", "x", "y", "z")"#));
    assert!(
        !f.bool_of(r#"add_anim_layer_blend_sample_3d("demo/walk", "nope", "idle", 0.0, 0.0, 0.0)"#)
    );
    assert!(!f.bool_of(r#"set_anim_layer_blend_value_3d("demo/walk", "nope", 0.0, 0.0, 0.0)"#));
}

#[test]
fn script_blend_api_returns_neutral_values_for_missing_target() {
    let mut f = fixture();
    // 实例不存在 / 层不存在：一律 false，不 panic
    assert!(!f.bool_of(r#"set_anim_layer_blend_1d("demo/walk", "nope", "speed")"#));
    assert!(!f.bool_of(r#"set_anim_layer_blend_2d("nobody", "nope", "x", "y")"#));
    assert!(!f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "nope", "idle", 0.0, 0.0)"#));
    assert!(!f.bool_of(r#"set_anim_layer_blend_value("demo/walk", "nope", 0.0, 0.0)"#));
    assert!(!f.bool_of(r#"clear_anim_layer_blend("demo/walk", "nope")"#));
    assert_eq!(f.string_of(r#"anim_layer_blend("demo/walk", "nope")"#), "");

    // 层在、但片段名解析不到：样本被拒，空间本身照样挂上
    assert!(f.bool_of(r#"add_anim_layer("demo/walk", "move", "idle", 1.0)"#));
    assert!(f.bool_of(r#"set_anim_layer_blend_1d("demo/walk", "move", "speed")"#));
    assert!(!f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "ghost", 0.0, 0.0)"#));
    let line = f.string_of(r#"anim_layer_blend("demo/walk", "move")"#);
    assert!(line.starts_with("1d(speed) n=0 drop=1 v=0.00"), "{line}");
}

// ---------------- 扩展五十一：剖分缓存（脏标记 + 延迟提交） ----------------

#[test]
fn script_blend_stale_flag_and_manual_build() {
    let mut f = fixture();
    assert!(f.bool_of(r#"add_anim_layer("demo/walk", "move", "idle", 1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_clip("demo/walk", "move", "atk", 0.0)"#));

    // 没挂空间：标记恒 false，手动提交也返回 false（探针不用先判空）
    assert!(!f.bool_of(r#"anim_layer_blend_stale("demo/walk", "move")"#));
    assert!(!f.bool_of(r#"build_anim_layer_blend("demo/walk", "move")"#));

    assert!(f.bool_of(r#"set_anim_layer_blend_2d("demo/walk", "move", "move.x", "move.y")"#));
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "idle", -1.0, -1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "atk", 1.0, -1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "idle", -1.0, 1.0)"#));

    // 加完样本：挂起中
    assert!(f.bool_of(r#"anim_layer_blend_stale("demo/walk", "move")"#));

    // 手动提交：第一次真的重建，第二次不重复付钱
    assert!(f.bool_of(r#"build_anim_layer_blend("demo/walk", "move")"#));
    assert!(!f.bool_of(r#"anim_layer_blend_stale("demo/walk", "move")"#));
    assert!(!f.bool_of(r#"build_anim_layer_blend("demo/walk", "move")"#));

    // 再补一个样本 → 又挂起，逐帧 advance 会替我们提交
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "atk", 1.0, 1.0)"#));
    assert!(f.bool_of(r#"anim_layer_blend_stale("demo/walk", "move")"#));
    f.advance(0.0);
    assert!(
        !f.bool_of(r#"anim_layer_blend_stale("demo/walk", "move")"#),
        "advance 应把挂起的剖分提交掉"
    );

    // 缺失目标仍是中性值
    assert!(!f.bool_of(r#"anim_layer_blend_stale("demo/walk", "nope")"#));
    assert!(!f.bool_of(r#"build_anim_layer_blend("nobody", "nope")"#));
}

#[test]
fn script_blend_stale_keeps_weights_and_pose_unchanged() {
    // 挂起只推迟「建剖分」，不能推迟「算权重」——回显与姿态都得先对。
    let mut f = fixture();
    assert!(f.bool_of(r#"add_anim_layer("demo/walk", "move", "idle", 1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_clip("demo/walk", "move", "atk", 0.0)"#));
    assert!(f.bool_of(r#"set_anim_layer_blend_2d("demo/walk", "move", "move.x", "move.y")"#));
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "idle", -1.0, -1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "atk", 1.0, -1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "idle", -1.0, 1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "atk", 1.0, 1.0)"#));
    assert!(f.bool_of(r#"set_anim_layer_blend_value("demo/walk", "move", 0.5, 0.5)"#));

    // 挂起状态下回显：拓扑数字已是最终值，权重也已是最终权重
    let stale_line = f.string_of(r#"anim_layer_blend("demo/walk", "move")"#);
    assert!(
        stale_line.contains("tri=2"),
        "挂起时也要报出 2 个三角形: {stale_line}"
    );
    assert!(f.bool_of(r#"anim_layer_blend_stale("demo/walk", "move")"#));

    // advance 提交剖分后再看：回显逐字相同
    f.advance(0.5);
    let committed_line = f.string_of(r#"anim_layer_blend("demo/walk", "move")"#);
    assert_eq!(stale_line, committed_line, "提交前后回显应逐字相同");

    // 姿态同口径：v=(0.5,0.5) 落在 atk 那侧的三角形里，atk=0.75、idle=0.25；
    // t=0.5 时 idle 的 arm 在 0、atk 的 arm 在 50 → 37.5
    let x = f.pos_x("arm");
    assert!(
        (x - 37.5).abs() < 1e-3,
        "挂起时姿态应与会话提交后一致，实际 {x}"
    );
}
