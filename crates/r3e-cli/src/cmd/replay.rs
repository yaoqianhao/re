//! `replay`：确定性回放（子进程复用 `r3e-replay`，不在本程序里重跑模拟）。
//!
//! # 为什么是子进程而不是「顺手实现一遍」
//!
//! 回放要的是一条**确定性的时间线**：同 seed 同输入必须得到同一个末哈希。
//! 这件事已经由 `r3e-replay` 独占实现（它不依赖引擎、不依赖窗口）。CLI 若自己再跑一遍，
//! 等于把「确定性」这件事分叉成两份实现——两份迟早不一致，而那时红的是 Gate 1，
//! 排查的人却不知道该信哪一份。
//!
//! # 找不到二进制时的三级退路
//!
//! `--replay-bin` → `$R3E_REPLAY_BIN` → 与本程序同目录的 `r3e-replay[.exe]` →
//! `cargo run -p r3e-replay --`（本地开发时零配置可用）。三级都没有才回退出码 4：
//! 「缺依赖」和「环境错误」必须分开，前者重试一万次也没用。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;

use crate::args::{Global, ReplayArgs};
use crate::exit;
use crate::output::{kv, Out};

/// 环境变量：显式指定 `r3e-replay` 路径。
const ENV_REPLAY_BIN: &str = "R3E_REPLAY_BIN";

/// 入口。
pub fn run(global: &Global, args: ReplayArgs) -> i32 {
    let out = Out::new(global);

    let Some(launcher) = resolve_launcher(&args) else {
        out.failure(
            "找不到 `r3e-replay` 二进制",
            &[
                "按顺序试过：`--replay-bin` → `$R3E_REPLAY_BIN` → 同目录 r3e-replay[.exe] → `cargo run`"
                    .to_string(),
                "修法：`cargo build -p r3e-replay` 后把二进制放到 r3e-cli 同目录，或用 `--replay-bin <path>` 指定"
                    .to_string(),
            ],
        );
        return exit::MISSING_DEP;
    };

    let mut command = Command::new(&launcher.program);
    command
        .args(&launcher.prefix)
        .arg("--seed")
        .arg(args.seed.to_string())
        .arg("--frames")
        .arg(args.frames.to_string())
        .arg("--hash-every")
        .arg(args.hash_every.to_string());

    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            out.failure(
                &format!("无法启动 {}：{error}", launcher.program.display()),
                &[
                    "确认该路径可执行；给了 `--replay-bin` 时以它为准（不会回退到自动探测）"
                        .to_string(),
                ],
            );
            return exit::MISSING_DEP;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        let mut details = vec![format!("退出状态：{}", describe_status(&output.status))];
        if let Some(last) = last_line(&stderr) {
            details.push(format!("stderr：{last}"));
        }
        out.failure("r3e-replay 执行失败", &details);
        return exit::NEGATIVE;
    }

    let Some(final_hash) = extract_final(&stdout) else {
        out.failure(
            "r3e-replay 的输出里没有 `final=` 末哈希",
            &[
                "回放契约是 `[replay] done ... final=<hex>`；拿不到它就无法做金标比对".to_string(),
                format!(
                    "stdout 末行：{}",
                    last_line(&stdout).unwrap_or_else(|| "（空）".to_string())
                ),
            ],
        );
        return exit::NEGATIVE;
    };

    // `verify` 的 `stateHash` 带 `0x`、`r3e-replay` 的 `final` 不带——用户复制粘贴时
    // 多半连前缀一起带进来，这里统一剥掉再比，否则「金标一致」会被一个前缀判成不一致。
    let matched = args
        .expect
        .as_deref()
        .map(|expect| strip_hex_prefix(expect.trim()).eq_ignore_ascii_case(&final_hash));

    let payload = json!({
        "final": final_hash,
        "expect": args.expect,
        "matched": matched,
        "seed": args.seed,
        "frames": args.frames,
        "hashEvery": args.hash_every,
        "binary": launcher.describe(),
        "summary": last_line(&stdout),
    });

    let mut pairs: Vec<(String, String)> = vec![
        ("final".to_string(), final_hash.clone()),
        (
            "参数".to_string(),
            format!(
                "seed {} / frames {} / hash-every {}",
                args.seed, args.frames, args.hash_every
            ),
        ),
        ("来源".to_string(), launcher.describe()),
    ];
    if let Some(expect) = &args.expect {
        pairs.push((
            "期望".to_string(),
            format!(
                "{expect} → {}",
                if matched == Some(true) {
                    "一致"
                } else {
                    "不一致（确定性被破坏，或参数/输入不同）"
                }
            ),
        ));
    }
    if let Some(summary) = last_line(&stdout) {
        pairs.push(("回放摘要".to_string(), summary));
    }

    out.success(&payload, &format!("确定性回放完成\n{}", kv(&pairs)));

    if matched == Some(false) {
        exit::NEGATIVE
    } else {
        exit::OK
    }
}

/// 怎么启动 `r3e-replay`。
struct Launcher {
    program: PathBuf,
    /// `cargo run` 形态需要的固定前缀参数（直接跑二进制时为空）。
    prefix: Vec<String>,
    /// 输出里展示的来源说明。
    source: &'static str,
}

impl Launcher {
    fn describe(&self) -> String {
        let mut text = format!("{}（{}）", self.program.display(), self.source);
        if !self.prefix.is_empty() {
            text.push_str(&format!(" {}", self.prefix.join(" ")));
        }
        text
    }
}

fn resolve_launcher(args: &ReplayArgs) -> Option<Launcher> {
    if let Some(bin) = &args.bin {
        // 显式指定就只认它：静默回退会让「我明明指了这个二进制」变成一场空排查。
        return Some(Launcher {
            program: bin.clone(),
            prefix: Vec::new(),
            source: "--replay-bin",
        });
    }

    if let Some(raw) = std::env::var_os(ENV_REPLAY_BIN) {
        let path = PathBuf::from(raw);
        if path.is_file() {
            return Some(Launcher {
                program: path,
                prefix: Vec::new(),
                source: ENV_REPLAY_BIN,
            });
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["r3e-replay.exe", "r3e-replay"] {
                let candidate = dir.join(name);
                if is_executable_file(&candidate) {
                    return Some(Launcher {
                        program: candidate,
                        prefix: Vec::new(),
                        source: "同目录",
                    });
                }
            }
        }
    }

    if cargo_available() {
        return Some(Launcher {
            program: PathBuf::from("cargo"),
            prefix: vec![
                "run".to_string(),
                "--quiet".to_string(),
                "-p".to_string(),
                "r3e-replay".to_string(),
                "--".to_string(),
            ],
            source: "cargo run（开发态兜底，较慢）",
        });
    }

    None
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn cargo_available() -> bool {
    Command::new("cargo")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// 从 stdout 里取末哈希：取**最后一条** `final=`（多轮回放时以最终轮为准）。
fn extract_final(stdout: &str) -> Option<String> {
    const MARKER: &str = "final=";
    stdout.lines().rev().find_map(|line| {
        let index = line.rfind(MARKER)?;
        let rest = strip_hex_prefix(line[index + MARKER.len()..].trim_start());
        let hex: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        if hex.is_empty() {
            None
        } else {
            Some(hex)
        }
    })
}

/// 去掉可选的 `0x` / `0X` 前缀（回放输出不带，引擎 stateHash 带）。
fn strip_hex_prefix(text: &str) -> &str {
    text.strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text)
}

fn last_line(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

fn describe_status(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => code.to_string(),
        None => "被信号终止".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_hash_is_taken_from_the_last_round() {
        let stdout = "[replay] round 1 final=0xaaaa\nnoise\n[replay] done seed=42 final=0x6ee98c0a9dbf6555\n";
        assert_eq!(
            extract_final(stdout).as_deref(),
            Some("6ee98c0a9dbf6555"),
            "多轮回放以最终轮为准"
        );
    }

    #[test]
    fn final_hash_stops_at_non_hex() {
        let stdout = "[replay] done final=deadbeef (frames=600)";
        assert_eq!(extract_final(stdout).as_deref(), Some("deadbeef"));
    }

    #[test]
    fn missing_final_marker_returns_none() {
        assert_eq!(extract_final("[replay] done frames=600"), None);
        assert_eq!(extract_final("[replay] final= (空)"), None);
        assert_eq!(extract_final(""), None);
    }

    #[test]
    fn last_line_skips_trailing_blanks() {
        assert_eq!(last_line("a\n\n  b  \n\n").as_deref(), Some("b"));
        assert_eq!(last_line("   \n\n"), None);
    }

    #[test]
    fn expect_accepts_the_state_hash_format() {
        // 从 `verify` 输出里整段复制 `0x6ee9…` 也必须能比中：前缀不属于哈希内容。
        assert_eq!(strip_hex_prefix("0x6ee98c0a"), "6ee98c0a");
        assert_eq!(strip_hex_prefix("0X6ee98c0a"), "6ee98c0a");
        assert_eq!(strip_hex_prefix("6ee98c0a"), "6ee98c0a");
        assert_eq!(
            extract_final("[replay] final=0xdeadbeef").as_deref(),
            Some("deadbeef")
        );
    }

    #[test]
    fn explicit_bin_never_falls_back() {
        // 给了 `--replay-bin` 就只认它：静默回退会让「我明明指了这个二进制」变成一场空排查。
        let args = ReplayArgs {
            bin: Some(PathBuf::from("Z:/nope/r3e-replay.exe")),
            ..ReplayArgs::default()
        };
        let launcher = resolve_launcher(&args).expect("显式路径必须原样返回");
        assert_eq!(launcher.program, PathBuf::from("Z:/nope/r3e-replay.exe"));
        assert!(launcher.prefix.is_empty());
        assert_eq!(launcher.source, "--replay-bin");
    }
}
