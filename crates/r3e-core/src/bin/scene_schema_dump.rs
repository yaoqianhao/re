//! 把 [`r3e_core::scene_schema`] 导出的场景 schema 落盘。
//!
//! 用法（仓库根目录执行）：
//!
//! ```text
//! cargo run -p r3e-core --bin scene_schema_dump            # 写 assets/scene.schema.json
//! cargo run -p r3e-core --bin scene_schema_dump -- <路径>  # 写到指定路径
//! cargo run -p r3e-core --bin scene_schema_dump -- --stdout # 只打印不落盘
//! ```
//!
//! 防漂移测试（`crates/r3e-core/tests/scene_schema.rs`）以本程序的输出为基准，
//! 因此「改了 serde 结构」时跑一次本程序即可让测试重新变绿。

use std::path::PathBuf;
use std::process::ExitCode;

use r3e_core::scene_schema::{scene_schema_pretty, SCENE_SCHEMA_PATH};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let text = scene_schema_pretty();

    if args.iter().any(|a| a == "--stdout") {
        print!("{text}");
        return ExitCode::SUCCESS;
    }

    let explicit = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(PathBuf::from);
    // 默认写到 crate 相对路径解析出的仓库根下的 assets/，避免受当前工作目录影响。
    let path = explicit.unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .map(|root| root.join(SCENE_SCHEMA_PATH))
            .unwrap_or_else(|| PathBuf::from(SCENE_SCHEMA_PATH))
    });

    if let Some(dir) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(dir) {
            eprintln!("创建目录失败 {}: {err}", dir.display());
            return ExitCode::FAILURE;
        }
    }
    match std::fs::write(&path, text.as_bytes()) {
        Ok(()) => {
            println!(
                "场景 schema 已写入 {}（{} 字节）",
                path.display(),
                text.len()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("写入失败 {}: {err}", path.display());
            ExitCode::FAILURE
        }
    }
}
