//! 回归：R/T 传送必须活过**同帧的静止贴地校正**（H-01B 现象③）。
//!
//! 缺陷形态：`on_update` 先 `get_pos` 把帧首坐标缓存到 `p`，R/T 的 `set_pos` 夹在中间，
//! 帧末的静止贴地校正又用旧 `p` 写一次位置——「传送 → 立刻被写回旧坐标」，
//! 表现为按 R/T 完全没反应（100% 复现，与键盘链路无关）。
//!
//! 夹具直接用 `assets/labs/terrain_roam/scripts/roam.rhai` 本体（与运行时实验场同一份文件，
//! 免得脚本改了两边不一致）。物理后端换成一支持「向下射线恒命中 y=0 平面」的闭包替身：
//! 若沿用 `NullPhysicsHost`，`ground_at` 恒返回 -999，贴地校正分支压根不执行，
//! 缺陷会在「假绿」里溜过去——这正是本用例必须自带物理替身的原因。

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use r3e_core::host::{HitDto, PhysReq, PhysRsp, PhysicsHost};
use r3e_core::{InputState, KeyCode};
use r3e_doc::Document;
use r3e_script::ScriptHost;

/// 单帧步长（60 fps）。
const DT: f64 = 0.016;

/// roam.rhai 的绝对路径：与运行时实验场共用同一份脚本。
fn roam_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/labs/terrain_roam/scripts/roam.rhai")
}

/// 地面替身：高度只随 x 变化（`height_of_x(x)`），射线一律竖直向下。
///
/// 只覆盖 `Raycast`，其余请求一律 `Unit`（等同「没有这项能力」），
/// 与 null 后端同一口径，保证用例只走「贴地」这一条路径。
fn ground_of(height_of_x: fn(f64) -> f64) -> Rc<dyn PhysicsHost> {
    Rc::new(move |req: PhysReq<'_>| match req {
        PhysReq::Raycast { origin, dir, max } => {
            if dir[1] > -0.99 {
                return PhysRsp::Hit(None);
            }
            let ground = height_of_x(origin[0]);
            let dist = origin[1] - ground;
            if dist < 0.0 || dist > max {
                return PhysRsp::Hit(None);
            }
            PhysRsp::Hit(Some(HitDto {
                node: 1,
                handle: 0,
                distance: dist,
                point: [origin[0], ground, origin[2]],
                normal: [0.0, 1.0, 0.0],
                inside: false,
            }))
        }
        _ => PhysRsp::Unit,
    })
}

/// 装配宿主：`player` 放在 `pos`，地面按 `height_of_x` 给，载入 roam.rhai（会跑 `on_start`）。
fn spawn(
    pos: [f32; 3],
    height_of_x: fn(f64) -> f64,
) -> (ScriptHost, Rc<RefCell<InputState>>, Rc<RefCell<Document>>) {
    let scene_src = format!(
        r#"{{"name":"teleport","objects":[{{"name":"player","position":[{},{},{}]}}]}}"#,
        pos[0], pos[1], pos[2]
    );
    let doc = Rc::new(RefCell::new(
        Document::from_json_str(&scene_src).expect("最小传送场景应当解析成功"),
    ));
    let input = Rc::new(RefCell::new(InputState::default()));
    let mut host = ScriptHost::new(doc.clone(), ground_of(height_of_x), input.clone());
    host.load_file(&roam_script())
        .expect("roam.rhai 应当编译通过并跑完 on_start");
    (host, input, doc)
}

/// 跑 `frames` 帧（每帧末清输入边沿，模拟 app 的帧循环）。
fn steps(
    host: &mut ScriptHost,
    input: &Rc<RefCell<InputState>>,
    frames: usize,
    first_frame: u64,
) -> u64 {
    let mut frame = first_frame;
    for i in 0..frames {
        host.tick(DT, DT * (frame as f64), frame, 60.0);
        input.borrow_mut().end_frame();
        frame += 1;
        let _ = i;
    }
    frame
}

/// 读 `player` 当前位置。
fn pos(doc: &Rc<RefCell<Document>>) -> [f32; 3] {
    let d = doc.borrow();
    let n = d.find("player").expect("场景里应当有 player");
    [
        n.transform.position.x,
        n.transform.position.y,
        n.transform.position.z,
    ]
}

fn set_key(input: &Rc<RefCell<InputState>>, key: KeyCode, down: bool) {
    input.borrow_mut().keyboard.set(key, down);
}

fn assert_near(actual: f32, expect: f32, what: &str) {
    assert!(
        (actual - expect).abs() < 1e-3,
        "{what}：期望 {expect}，实际 {actual}"
    );
}

/// 平地（y = 0）：R 键落点用这套地面。
fn flat(_x: f64) -> f64 {
    0.0
}

/// 坡顶台地（x ≥ 20 起高 28.0，其余平地）：故意**不等于**脚本里硬编码的 29.193，
/// 这样「传送后按新坐标重新打射线」才与「保留硬编码落点高度」区分得开。
fn summit(x: f64) -> f64 {
    if x >= 20.0 {
        28.0
    } else {
        0.0
    }
}

#[test]
fn 按r键传送到出生点当帧生效且不被贴地校正写回() {
    let (mut host, input, doc) = spawn([0.0, 0.0, 0.0], flat);
    // 两帧预热：让 on_start / 静止贴地校正都进入稳态（脚底抬到 0.9）。
    let mut frame = steps(&mut host, &input, 2, 1);
    let p = pos(&doc);
    assert_near(p[0], 0.0, "预热后 x");
    assert_near(p[1], 0.9, "预热后脚底");

    // R 键：脚本把角色写到 (-90, ·, 20)，同帧末尾不许再用旧坐标写回。
    set_key(&input, KeyCode::R, true);
    frame = steps(&mut host, &input, 1, frame);
    let p = pos(&doc);
    assert_near(
        p[0],
        -90.0,
        "R 传送当帧的 x（被同帧贴地校正覆盖即为此处失败）",
    );
    assert_near(p[2], 20.0, "R 传送当帧的 z");
    assert_near(p[1], 0.9, "R 落点脚底（传送分支按新坐标贴地）");
    assert!(host.last_error.is_none(), "脚本报错: {:?}", host.last_error);

    // 松键后再跑两帧：传送结果要留得住，不能被后续帧弹回原点。
    set_key(&input, KeyCode::R, false);
    let _ = steps(&mut host, &input, 2, frame);
    let p = pos(&doc);
    assert_near(p[0], -90.0, "松键后仍应停在出生点 x");
    assert_near(p[2], 20.0, "松键后仍应停在出生点 z");
}

#[test]
fn 按t键传送到坡顶当帧生效且按新坐标重新贴地() {
    let (mut host, input, doc) = spawn([-90.0, 0.0, 20.0], summit);
    let mut frame = steps(&mut host, &input, 2, 1);

    set_key(&input, KeyCode::T, true);
    frame = steps(&mut host, &input, 1, frame);
    let p = pos(&doc);
    assert_near(
        p[0],
        50.0,
        "t 传送当帧的 x（被同帧贴地校正覆盖即为此处失败）",
    );
    assert_near(p[2], 20.0, "t 传送当帧的 z");
    // 脚本里硬编码的落点是 29.193 + 0.9；替身地面在坡顶只有 28.0。
    // 落点高度必须是 28.9（传送分支按**新坐标**重新打了射线），
    // 若停在 30.093 就说明拿的是硬编码值、贴地根本没对新坐标跑。
    assert_near(p[1], 28.9, "t 落点脚底应按新坐标重新贴地");
    assert!(host.last_error.is_none(), "脚本报错: {:?}", host.last_error);

    set_key(&input, KeyCode::T, false);
    let _ = steps(&mut host, &input, 2, frame);
    let p = pos(&doc);
    assert_near(p[0], 50.0, "松键后仍应停在坡顶 x");
}

#[test]
fn 静止贴地校正本身没有被传送守卫关掉() {
    // 角色被摆到 5.0 高处、不按任何键：静止贴地校正仍须把脚底拉回地面 + 0.9。
    let (mut host, input, doc) = spawn([12.0, 5.0, -7.0], flat);
    let _ = steps(&mut host, &input, 1, 1);
    let p = pos(&doc);
    assert_near(p[0], 12.0, "校正不应改动 x");
    assert_near(p[2], -7.0, "校正不应改动 z");
    assert_near(p[1], 0.9, "静止贴地校正应把脚底拉到地面 + 0.9");
}
