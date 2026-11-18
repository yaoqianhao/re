//! `r3e-cli` 入口。
//!
//! # 这个程序只做三件事
//!
//! 1. **把命令行翻成一次 RPC**（`args`），不持有任何场景知识；
//! 2. **把回包翻成「人能照着做」的文本或一行 JSON**（`output`）；
//! 3. **把失败翻成稳定的退出码**（`errmap` + `exit`）。
//!
//! 场景的真相在引擎里，命令契约在 `r3e-protocol` 里。CLI 一旦开始自己判断
//! 「这个节点该不该动」，AI 就会拿到两份不一致的世界模型——所以这里没有业务逻辑。
//!
//! # 依赖红线
//!
//! 只依赖 `r3e-protocol`（+ `serde_json`）。不引 `r3e-core` / `r3e-doc` / `r3e-editor-host`
//! 是 `xtask` 的 `dep-check` 硬门禁：CLI 要能脱离引擎源码独立分发。

mod args;
mod cmd;
mod errmap;
mod exit;
mod output;
mod state;

use args::Cmd;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // 用 exit 而不是 return：`run` 里可能已经铺了若干 stderr，退出码必须原样带出去。
    std::process::exit(run(argv));
}

/// 跑一次完整调用，返回退出码（不发散：**所有**失败路径都归到 0..=4）。
fn run(argv: Vec<String>) -> i32 {
    let invocation = match args::parse(argv) {
        Ok(invocation) => invocation,
        Err(err) => {
            eprintln!("参数错误：{}", err.message);
            eprintln!("用法：r3e-cli <子命令> [参数]；`r3e-cli --help` 看全部");
            return exit::USAGE;
        }
    };

    if matches!(invocation.cmd, Cmd::Help) {
        print!("{}", args::help_text());
        return exit::OK;
    }

    cmd::run(&invocation.global, invocation.cmd)
}
