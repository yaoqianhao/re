//! 补 M2-E 遗留缺口的判据：① 声明式 prefab 实例在脚本跑完后被真正展开成节点；
//! ④ `println` 在 Rhai 侧可用。
//!
//! # 为什么判据落在这里（`r3e-script`）而不是 `App::new`
//!
//! 模板的唯一来源是脚本的 `prefab_define`（`app` 侧没有模板来源），所以「模板到齐」
//! 的那一刻就是 `on_start` 跑完的那一刻——接线点在脚本宿主里，判据也就该在脚本层锁。
//! 三条真实路径分别对应下面三个用例：
//!
//! | 路径 | 触发方式 | 用例 |
//! |---|---|---|
//! | 启动（`App::new` → `load_file` → `load_str`） | 场景先带着 `instances`，脚本后跑 | [`load_str_expands_declared_instances`] |
//! | F5 / 编辑器改 scene.json（`rebuild_scene`） | 先载脚本，再换带 `instances` 的场景 | [`rebuild_scene_expands_declared_instances`] |
//! | 失败面（模板没登记） | 场景有实例、脚本不定义模板 | [`missing_template_keeps_record_and_load_succeeds`] |
//!
//! # 判据口径
//!
//! 1. **节点真的出现了**：`Scene::find` 能找到同名节点，且世界位置等于实例覆盖里的
//!    `position`（`spawn` 只写局部变换，世界矩阵得靠 `update_world_transforms` 补，
//!    这条断言顺带把「同帧能读到正确世界位置」也锁住）。
//! 2. **实例记录不丢**：`scene.instances` 仍保留这条记录（重组 `to_scene_file` 时
//!    实例段不能因为「已经展开过」就消失）。
//! 3. **幂等**：展开过的实例再展开一次返回 `0`，节点数不变（热重载会反复走这条路）。
//! 4. **失败不冒泡**：模板缺失时 `load_str` 仍返回 `Ok`，实例记录原样留在表里，
//!    只是没有节点——不能把「模型没出来」升级成「引擎起不来」。

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use r3e_core::{InputState, Scene};
use r3e_doc::Document;
use r3e_physics::{Physics, PhysicsFacade};
use r3e_script::ScriptHost;

/// 模板 id（场景文件的 `instances[].template` 与脚本 `prefab_define` 的 id 必须一致）。
const TEMPLATE_ID: &str = "demo.crate";

/// 模板体：与 `output/prefab_demo_crate.prefab.json` 同口径的最小版（proto 位置故意
/// 与下面实例的覆盖不同，用来证明「节点位置来自覆盖，不是来自模板」）。
const TEMPLATE_JSON: &str = r#"{
  "id": "demo.crate",
  "schema": 1,
  "tags": ["prop", "demo", "crate"],
  "proto": {
    "name": "crate",
    "mesh": "cube",
    "position": [-3.0, 0.8, 0.0],
    "rotation": [0.0, 0.0, 0.0],
    "scale": [1.6, 1.6, 1.6],
    "color": [0.72, 0.55, 0.34, 1.0],
    "visible": true,
    "tags": ["prop", "crate"],
    "interact_radius": 1.2
  }
}"#;

/// 带一条声明式实例的场景：`crate_declared` 只有记录，没有节点（这正是缺口①的现场）。
const SCENE_WITH_DECLARED: &str = r#"{
  "name": "prefab_boot",
  "objects": [
    { "name": "ground", "mesh": "plane", "position": [0.0, 0.0, 0.0] }
  ],
  "instances": [
    {
      "name": "crate_declared",
      "template": "demo.crate",
      "overrides": {
        "position": [3.0, 0.8, 3.0],
        "color": [0.85, 0.35, 0.30, 1.0]
      }
    }
  ]
}"#;

/// 只有地板的空场景（用来单独看「没有声明实例时什么都不发生」）。
const SCENE_EMPTY: &str = r#"{
  "name": "prefab_boot_empty",
  "objects": [
    { "name": "ground", "mesh": "plane", "position": [0.0, 0.0, 0.0] }
  ]
}"#;

/// 带**父子**声明实例的场景：`crate_stack` 的 `parent` 指向同一段里的 `crate_declared`，
/// `position` 是相对父节点的局部坐标（模板 proto 的 scale 是 1.6，所以世界 y = 0.8 + 1.6×1.6）。
const SCENE_WITH_DECLARED_CHILD: &str = r#"{
  "name": "prefab_boot_child",
  "objects": [
    { "name": "ground", "mesh": "plane", "position": [0.0, 0.0, 0.0] }
  ],
  "instances": [
    {
      "name": "crate_declared",
      "template": "demo.crate",
      "overrides": { "position": [3.0, 0.8, 3.0] }
    },
    {
      "name": "crate_stack",
      "template": "demo.crate",
      "parent": "crate_declared",
      "overrides": { "position": [0.0, 1.6, 0.0] }
    }
  ]
}"#;

/// 把模板写到系统临时目录，返回**正斜杠**路径。
///
/// Rhai 字符串里反斜杠是转义符，Windows 路径必须换成 `/` 直接塞进脚本；文件名带
/// `tag` 与 pid，避免并行跑测试时互踩。
fn write_template(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "r3e_prefab_boot_{}_{tag}.prefab.json",
        std::process::id()
    ));
    std::fs::write(&path, TEMPLATE_JSON).expect("写模板文件");
    path
}

/// 把磁盘路径转成能直接嵌进 Rhai 源码的字面量内容（正斜杠）。
fn rhai_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// 用给定场景建脚本宿主（与运行时装配同一个构造入口）。
fn host_with_scene(json: &str) -> (ScriptHost, Rc<RefCell<Document>>) {
    let scene: Scene = r3e_doc::parse_scene(json).expect("场景解析");
    let doc = Rc::new(RefCell::new(Document::new(scene)));
    let physics = Rc::new(RefCell::new(Physics::new()));
    let input = Rc::new(RefCell::new(InputState::default()));
    let host = ScriptHost::new(doc.clone(), PhysicsFacade::wrap(physics), input);
    (host, doc)
}

/// 「登记模板」的 `on_start`：demo 脚本里 `prefab_define` 就是这么写的。
fn define_script(template: &Path) -> String {
    format!(
        "fn on_start() {{\n    prefab_define(\"{TEMPLATE_ID}\", \"{}\");\n}}\n",
        rhai_path(template)
    )
}

/// 判据①-a（启动路径）：`load_str` 跑完 `on_start` 后，声明实例变成真节点。
#[test]
fn load_str_expands_declared_instances() {
    let template = write_template("load_str");
    let (mut host, doc) = host_with_scene(SCENE_WITH_DECLARED);
    host.load_str(&define_script(&template), "boot.rhai")
        .expect("脚本载入不应失败");

    let scene = doc.borrow();
    let node = scene
        .find("crate_declared")
        .expect("声明实例应被展开成节点");
    // 位置来自实例覆盖 (3.0, 0.8, 3.0)，不是模板 proto 的 (-3.0, 0.8, 0.0)——
    // 顺带证明「覆盖在展开时就生效」，不是事后再 sync 一遍。
    let world = node.world_position();
    assert!((world.x - 3.0).abs() < 1e-4, "世界 x = {}", world.x);
    assert!((world.y - 0.8).abs() < 1e-4, "世界 y = {}", world.y);
    assert!((world.z - 3.0).abs() < 1e-4, "世界 z = {}", world.z);
    // 实例记录不能因为「已展开」就被删掉：`to_scene_file` 还要把它写回磁盘。
    assert_eq!(scene.instances.len(), 1, "实例记录必须在表里");
    assert_eq!(scene.instances[0].name, "crate_declared");
}

/// 判据①-b（重建路径）：`rebuild_scene` 换进带实例的场景后同样展开。
///
/// 这条与启动路径是**两条独立入口**（`App` 里 F5 / 改 scene.json 走这里），
/// 只用一条用例盖不住。
#[test]
fn rebuild_scene_expands_declared_instances() {
    let template = write_template("rebuild");
    let (mut host, doc) = host_with_scene(SCENE_EMPTY);
    host.load_str(&define_script(&template), "boot.rhai")
        .expect("脚本载入不应失败");
    assert!(
        doc.borrow().find("crate_declared").is_none(),
        "空场景本来就没有实例节点"
    );

    host.rebuild_scene(SCENE_WITH_DECLARED)
        .expect("场景重建不应失败");
    let scene = doc.borrow();
    assert!(
        scene.find("crate_declared").is_some(),
        "重建后声明实例应被展开"
    );
    assert_eq!(scene.instances.len(), 1);
}

/// 判据①-c（幂等）：再展开一次返回 0、节点数不变。
///
/// 热重载每次都会走「替换场景 → 跑脚本 → 展开」，如果这里不幂等，节点会成倍长。
#[test]
fn expand_declared_prefabs_is_idempotent() {
    let template = write_template("idempotent");
    let (mut host, doc) = host_with_scene(SCENE_WITH_DECLARED);
    host.load_str(&define_script(&template), "boot.rhai")
        .expect("脚本载入不应失败");
    let nodes_after_load = doc.borrow().len();

    assert_eq!(
        host.expand_declared_prefabs(),
        0,
        "已展开的实例不该被重复展开"
    );
    assert_eq!(doc.borrow().len(), nodes_after_load, "节点数不该涨");
}

/// 判据①-e（父子声明）：`instances[].parent` 在展开时生效，子实例挂到父节点下，
/// 且它的 `position` 按**局部坐标**参与世界矩阵合成。
///
/// 这条锁的是 demo 里 `crate_stack` 那条路：父与子同在一段 `instances` 里、父写在前面，
/// 展开必须按声明顺序逐个来，否则子实例会因父节点还不存在而被拒。
#[test]
fn declared_child_instance_is_parented_and_local_position_composes() {
    let template = write_template("declared_child");
    let (mut host, doc) = host_with_scene(SCENE_WITH_DECLARED_CHILD);
    host.load_str(&define_script(&template), "boot.rhai")
        .expect("脚本载入不应失败");

    let scene = doc.borrow();
    let parent = scene.find("crate_declared").expect("父实例应被展开");
    let child = scene.find("crate_stack").expect("子实例应被展开");
    assert_eq!(
        child.parent,
        Some(parent.id),
        "子实例的 parent 必须指向父节点"
    );
    assert!(
        parent.children.contains(&child.id),
        "父节点的 children 里要有子实例"
    );
    // 局部 (0, 1.6, 0) 在父缩放 1.6 下 → 世界 y = 0.8 + 2.56。
    let world = child.world_position();
    assert!((world.x - 3.0).abs() < 1e-4, "世界 x = {}", world.x);
    assert!((world.y - 3.36).abs() < 1e-4, "世界 y = {}", world.y);
    assert!((world.z - 3.0).abs() < 1e-4, "世界 z = {}", world.z);
    // 两条实例记录都留着。
    assert_eq!(scene.instances.len(), 2);
}

/// 判据①-d（失败面）：模板没登记时脚本载入仍成功，实例记录保留但无节点。
///
/// 失败策略是「打日志 + 保留记录」，不是「整段启动失败」：模板缺失是内容问题，
/// 引擎该照常起来，让用户还能用控制台把模板补登记进去。
#[test]
fn missing_template_keeps_record_and_load_succeeds() {
    let (mut host, doc) = host_with_scene(SCENE_WITH_DECLARED);
    host.load_str("fn on_start() {\n}\n", "no_define.rhai")
        .expect("模板缺失不该让脚本载入失败");

    assert_eq!(host.expand_declared_prefabs(), 0, "没模板必然一条都展不开");
    let scene = doc.borrow();
    assert!(
        scene.find("crate_declared").is_none(),
        "展不开就不该凭空造节点"
    );
    assert_eq!(scene.instances.len(), 1, "实例记录必须留着，供校验器报警");
}

/// 判据④：`println` 与 `print` 在 Rhai 侧都能调（原先 `println` 报 Function not found）。
///
/// 只断言「能跑通」不比对输出：输出被 `engine.on_print` 收走打到 stdout，抓不到；
/// 「带换行」由实现里的 `println!` 保证，属实现细节，用集成测试断 stdio 不值当。
#[test]
fn println_is_bound_like_print() {
    let (mut host, _doc) = host_with_scene(SCENE_EMPTY);
    for src in [
        "println(\"boot.rhai: println 可用\");",
        "println(42);",
        "println(true);",
        "print(\"print 仍然可用\");",
    ] {
        let _ = host
            .eval(src)
            .unwrap_or_else(|err| panic!("`{src}` 执行失败: {err}"));
    }
    // 脚本里最常见的形态：函数体里写 println 后正常返回。
    host.load_str(
        "fn on_start() {\n    println(\"on_start 里的 println\");\n}\n",
        "println.rhai",
    )
    .expect("含 println 的脚本应能载入");
}
