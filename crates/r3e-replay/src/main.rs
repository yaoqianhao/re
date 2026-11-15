//! `r3e-replay` 命令行：跑一遍确定性回放并按契约格式打印。
//!
//! 输出契约（`xtask replay` 逐行比对，M1-S5 的 `r3e-cli replay` 直接复用）：
//!
//! ```text
//! [replay] seed=42 frames=600 hash_every=60 dt=0.016667
//! frame 0060 world=<16 位十六进制> state=<16 位十六进制> nodes=60 depth=6
//! ...
//! [replay] done frames=600 commits=60 undo_depth=60 final=<16 位十六进制>
//! ```
//!
//! 一帧一行里除了哈希还带 `nodes` / `depth`：只比哈希时「两次错得一样」也会绿，
//! 带上可读量才能在分叉时一眼看出是哪一帧开始不对。

use std::process::ExitCode;

use r3e_replay::{run, ReplayConfig};

fn main() -> ExitCode {
    match real_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[replay] error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn real_main() -> Result<(), String> {
    let config = parse_args(std::env::args().skip(1))?;
    let report = run(&config).map_err(|e| e.to_string())?;

    println!("{}", report.config.banner());
    for sample in &report.samples {
        println!("{}", sample.line());
    }
    println!("{}", report.footer());
    Ok(())
}

/// 解析参数：`--seed/--frames/--hash-every <值>`，未知参数报错（避免拼错静默跑默认值）。
fn parse_args(args: impl Iterator<Item = String>) -> Result<ReplayConfig, String> {
    let mut config = ReplayConfig::default();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            if let Some(v) = inline.clone() {
                return Ok(v);
            }
            args.next()
                .ok_or_else(|| format!("{name} 缺少取值（用法：{name} <数字>）"))
        };
        match flag.as_str() {
            "--seed" => {
                config.seed = value("--seed")?
                    .parse()
                    .map_err(|_| "--seed 需要一个 u64".to_string())?;
            }
            "--frames" => {
                config.frames = value("--frames")?
                    .parse()
                    .map_err(|_| "--frames 需要一个 u64".to_string())?;
            }
            "--hash-every" => {
                config.hash_every = value("--hash-every")?
                    .parse()
                    .map_err(|_| "--hash-every 需要一个 u64".to_string())?;
            }
            "-h" | "--help" => {
                println!("用法: r3e-replay [--seed <u64>] [--frames <u64>] [--hash-every <u64>]");
                std::process::exit(0);
            }
            other => {
                return Err(format!(
                    "未知参数 {other}（--seed / --frames / --hash-every）"
                ))
            }
        }
    }
    if config.frames == 0 {
        return Err("--frames 必须大于 0".to_string());
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_documented_ones() {
        let config = parse_args(Vec::<String>::new().into_iter()).expect("默认参数");
        assert_eq!(config, ReplayConfig::default());
        assert_eq!(config.seed, 42);
        assert_eq!(config.frames, 600);
        assert_eq!(config.hash_every, 60);
    }

    #[test]
    fn accepts_both_forms() {
        let a = parse_args(
            ["--seed", "9", "--frames=30", "--hash-every", "5"]
                .map(String::from)
                .into_iter(),
        )
        .expect("两种写法都要认");
        assert_eq!((a.seed, a.frames, a.hash_every), (9, 30, 5));
    }

    #[test]
    fn rejects_unknown_flag_and_zero_frames() {
        assert!(parse_args(["--seeds", "1"].map(String::from).into_iter()).is_err());
        assert!(parse_args(["--frames", "0"].map(String::from).into_iter()).is_err());
    }
}
