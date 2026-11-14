//! r3e —— 用 Rust 写的最小可用 3D 游戏引擎（薄壳入口）。
//!
//! 这里只做三件事：初始化日志、把 `argv` 交给运行时、在有 `--rpc-port` 时挂上编辑器桥。
//! 窗口、主循环、时间步、无窗口工具路径都在 `r3e-runtime`；RPC 协议与其适配在
//! `r3e-editor-host`。三者之间的关系是单向的：runtime 不认识协议，editor-host 反过来
//! 依赖 runtime，本文件只是把两边接起来的最后几行。
//!
//! 全部命令行选项见 `r3e --help`。

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let root = r3e_runtime::find_project_root();
    let parsed = r3e_runtime::parse(std::env::args().skip(1).collect(), root);

    if parsed.help {
        r3e_runtime::print_usage();
        return Ok(());
    }

    // `--dump-asset` / `--dump-anim` / `--save-scene`：无窗口路径，跑完直接退出。
    if let Some(code) = r3e_runtime::run_offscreen_tools(&parsed) {
        if code != 0 {
            std::process::exit(code);
        }
        return Ok(());
    }

    // 编辑器桥是可选挂件：没给 `--rpc-port` 就是纯引擎进程。
    let editor: Option<Box<dyn r3e_runtime::EditorService>> = match parsed.cli.rpc_port {
        Some(port) => Some(r3e_editor_host::bind(port, parsed.cli.rpc_token.clone())?),
        None => None,
    };

    r3e_runtime::run(parsed, editor)
}
