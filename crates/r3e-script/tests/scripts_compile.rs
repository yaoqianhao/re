//! 仓库自带演示脚本的**编译守卫**。
//!
//! 为什么要这一条：Rhai 的编译期错误（例如一行里串太多 `+` 撞上
//! `Expression exceeds maximum complexity`）不会在 `cargo build` 里暴露，
//! 只在 `load_file` 时返回 `Err`；而 `r3e` 对载入失败只是打一行 `log::error!`
//! 继续跑。结果是「脚本写错了，窗口照开，画面没有任何变化」，很容易被当成渲染 bug。
//! 这里把 `scripts/*.rhai` 全部过一遍编译，拼写 / 复杂度 / 语法问题在 CI 阶段就炸掉。
//!
//! 只断言**能载入**，不断言 `on_start` 里做的事成功：部分演示脚本依赖具体资产与场景，
//! 在空场景下 `on_start` 内部报错是预期行为（那一类错误由各脚本自己的演示场景验证）。

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use r3e_core::{InputState, Scene};
use r3e_physics::Physics;
use r3e_script::ScriptHost;

/// `scripts/` 目录的绝对路径（crate 在 `crates/r3e-script`）。
fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("scripts")
}

#[test]
fn every_demo_script_compiles() {
    let dir = scripts_dir();
    assert!(dir.is_dir(), "找不到脚本目录: {}", dir.display());

    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("读取 scripts/ 失败")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "rhai").unwrap_or(false))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "{} 下没有任何 .rhai", dir.display());

    let mut failures = Vec::new();
    for path in &paths {
        let scene = Rc::new(RefCell::new(Scene::new()));
        let physics = Rc::new(RefCell::new(Physics::new()));
        let input = Rc::new(RefCell::new(InputState::default()));
        let mut host = ScriptHost::new(scene, physics, input);

        if let Err(e) = host.load_file(path) {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            failures.push(format!("{name}: {e}"));
        }
    }

    assert!(
        failures.is_empty(),
        "以下演示脚本无法载入（编译期就失败了，运行时只会打一行 error 而不中断）：\n{}",
        failures.join("\n")
    );
}
