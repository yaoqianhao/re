//! 错误码 → 「下一步做什么」。
//!
//! # 为什么值得一个文件
//!
//! 引擎回的 `-32020` 对人不构成信息，对 AI 更糟：它只会照着数字瞎猜是「没命中」
//! 还是「没开能力」，然后开始改场景而不是开开关。Gate 1 要求「AI 仅靠协议闭环」，
//! 这个闭环的第一步就是**看懂失败**——所以每个 `-320xx` 在这里都必须配一句
//! 「现在去做什么」，而不是把码原样透传。
//!
//! # 退出码的裁决也在这里
//!
//! 三类失败必须分得开（`docs/m1/M1_S5_PLAN.md` §3.3）：脚本要能按码分支，
//! 而不是 grep 输出文本。判定顺序是「先环境（3）→ 再用法（2）→ 其余业务否定（1）」。

use r3e_protocol::client::{nearest_methods, ClientError};
use r3e_protocol::{
    BACKEND_UNSUPPORTED, ENGINE_ERROR, INVALID_PARAMS, INVALID_REQUEST, MESH_SOURCE_UNAVAILABLE,
    METHOD_NOT_FOUND, NAV_NOT_BAKED, PARSE_ERROR, PHYSICS_DISABLED, RPC_TIMEOUT, SERVER_BUSY,
    TX_APPLY_FAILED, TX_BUSY, TX_CONFLICT, TX_NOT_FOUND, TX_REJECTED, UNAUTHORIZED,
};

use crate::exit;

/// 一个错误码对应的「下一步」。
///
/// 返回 `None` 表示这是本层不认识的码：仍然会打出来，但不假装知道怎么修。
pub fn guidance(code: i64) -> Option<&'static str> {
    let text = if code == i64::from(TX_NOT_FOUND) {
        "txId 不存在或该事务已终结：重新 `r3e-cli tx begin`；引擎重启后旧 txId 一律失效"
    } else if code == i64::from(TX_BUSY) {
        "已有一笔未决事务：先 `tx commit` 或 `tx rollback` 收掉它，同一文档同时只允许一笔"
    } else if code == i64::from(TX_REJECTED) {
        "意图被拒：op 不在可写域白名单，或参数过不了命令层校验——先用 `query describe` 看准现状，再对齐参数"
    } else if code == i64::from(TX_CONFLICT) {
        "基哈希已漂移（事务外有人改了文档）：`tx rollback` 后重开，不要强推"
    } else if code == i64::from(TX_APPLY_FAILED) {
        "提交期某条命令失败，整笔未落（原子性）：`tx rollback` 后修正那条命令重来"
    } else if code == i64::from(PHYSICS_DISABLED) {
        "物理总开关是关的：引擎启动时加 `--physics on`（开了才有物理层结论），或先 `tx apply physics.bind`"
    } else if code == i64::from(BACKEND_UNSUPPORTED) {
        "当前物理后端不申报这项查询能力：换后端，或改用 `physics.raycast` 这条通路"
    } else if code == i64::from(NAV_NOT_BAKED) {
        "导航网格还没烘焙：`nav.bake` 不在可写意图白名单里，CLI 触发不了——先在引擎侧完成烘焙再重跑；没烘焙时 `verify` 会把 nav 层写进 unchecked 并因此回退出码 1（不是通过）"
    } else if code == i64::from(MESH_SOURCE_UNAVAILABLE) {
        "宿主拿不到网格来源：`physics.bind` 之前先 `assets.load`，或指定网格提供者"
    } else if code == i64::from(METHOD_NOT_FOUND) {
        "方法名不存在：见下方最近邻候选；查场景用 `query.*`，改场景用 `tx apply <op>`"
    } else if code == i64::from(INVALID_PARAMS) {
        "参数不合法（必填缺失 / 类型不对 / 超范围）：对照 `r3e-protocol` 对应 dispatch 的参数校验"
    } else if code == i64::from(PARSE_ERROR) || code == i64::from(INVALID_REQUEST) {
        "请求形状不合法：这是客户端自身的 bug，请带着本命令的输出反馈"
    } else if code == i64::from(UNAUTHORIZED) {
        "鉴权失败：用 `--token <s>` 或环境变量 `R3E_RPC_TOKEN` 给出引擎启动时 `--rpc-token` 的同一个值"
    } else if code == i64::from(SERVER_BUSY) {
        "引擎过载（待处理队列满 / 连接数到顶）：稍后重试；批量写请合并成一条 `tx intent`"
    } else if code == i64::from(RPC_TIMEOUT) {
        "主线程超时：引擎卡顿或主循环已停——先确认引擎窗口还在响应"
    } else if code == i64::from(ENGINE_ERROR) {
        "引擎侧业务错误：按 message 修正对象名 / 参数后重试"
    } else {
        return None;
    };
    Some(text)
}

/// 退出码：环境（3）→ 用法（2）→ 业务否定（1）。
pub fn exit_code(error: &ClientError) -> i32 {
    if error.is_environment() {
        return exit::ENV;
    }
    match error.code() {
        Some(code)
            if code == i64::from(INVALID_PARAMS)
                || code == i64::from(METHOD_NOT_FOUND)
                || code == i64::from(PARSE_ERROR)
                || code == i64::from(INVALID_REQUEST) =>
        {
            exit::USAGE
        }
        _ => exit::NEGATIVE,
    }
}

/// 把错误翻成「一句话结论 + 若干行可操作细节」。
///
/// 细节里包含最近邻方法名：`-32601` 若只说「不存在」，调用方只能瞎改；给出
/// 三条候选，它下一次就能改对。
pub fn explain(error: &ClientError) -> (String, Vec<String>) {
    match error {
        ClientError::Transport(detail) => {
            let summary = format!("连不上引擎：{detail}");
            let details = vec![
                "确认引擎已启动且给了 `--rpc-port <PORT>`（没给端口时 RPC 根本不监听）".to_string(),
                "确认 `--addr` / `R3E_RPC_ADDR` 与 `--rpc-port` 指向同一地址（缺省 127.0.0.1:8787）"
                    .to_string(),
                "引擎设了 `--rpc-token` 时，客户端必须给同一个 `--token`（否则会看到 UNAUTHORIZED）"
                    .to_string(),
            ];
            (summary, details)
        }
        ClientError::Remote { code, message } => {
            let summary = format!("引擎拒绝（{code}）：{message}");
            let mut details = Vec::new();
            if let Some(hint) = guidance(*code) {
                details.push(hint.to_string());
            } else {
                details.push(format!(
                    "本层没有为 {code} 登记「下一步」：按引擎 message 处理，并把这条码反馈回来"
                ));
            }
            if *code == i64::from(METHOD_NOT_FOUND) {
                // 只有方法名错才给最近邻：其他错误给候选方法等于转移注意力。
                let near = nearest_methods(message, 3);
                if !near.is_empty() {
                    details.push(format!("最近的已知方法：{}", near.join(" / ")));
                }
            }
            (summary, details)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_code_has_guidance() {
        // 本层承诺登记「下一步」的全部错误码。
        let codes = [
            i64::from(TX_NOT_FOUND),
            i64::from(TX_BUSY),
            i64::from(TX_REJECTED),
            i64::from(TX_CONFLICT),
            i64::from(TX_APPLY_FAILED),
            i64::from(PHYSICS_DISABLED),
            i64::from(BACKEND_UNSUPPORTED),
            i64::from(NAV_NOT_BAKED),
            i64::from(MESH_SOURCE_UNAVAILABLE),
            i64::from(METHOD_NOT_FOUND),
            i64::from(INVALID_PARAMS),
            i64::from(PARSE_ERROR),
            i64::from(INVALID_REQUEST),
            i64::from(UNAUTHORIZED),
            i64::from(SERVER_BUSY),
            i64::from(RPC_TIMEOUT),
        ];
        for code in codes {
            let text = guidance(code).unwrap_or_else(|| panic!("{code} 缺 guidance"));
            assert!(!text.trim().is_empty(), "{code} 的 guidance 是空串");
        }
    }

    #[test]
    fn unknown_code_is_not_faked() {
        // 不认识的码要老实说不知道：编一句「先检查网络」比不给还坏。
        assert_eq!(guidance(0), None);
        assert_eq!(guidance(-32024), None);
        assert_eq!(guidance(42), None);
    }

    #[test]
    fn exit_codes_separate_environment_from_usage_from_negative() {
        let transport = ClientError::Transport("连接被拒绝".to_string());
        assert_eq!(exit_code(&transport), exit::ENV);

        for code in [UNAUTHORIZED, SERVER_BUSY, RPC_TIMEOUT] {
            let error = ClientError::Remote {
                code: i64::from(code),
                message: String::new(),
            };
            assert_eq!(exit_code(&error), exit::ENV, "{code} 属于环境问题");
        }

        for code in [
            INVALID_PARAMS,
            METHOD_NOT_FOUND,
            PARSE_ERROR,
            INVALID_REQUEST,
        ] {
            let error = ClientError::Remote {
                code: i64::from(code),
                message: String::new(),
            };
            assert_eq!(exit_code(&error), exit::USAGE, "{code} 属于用法问题");
        }

        for code in [TX_BUSY, TX_NOT_FOUND, TX_REJECTED, ENGINE_ERROR] {
            let error = ClientError::Remote {
                code: i64::from(code),
                message: String::new(),
            };
            assert_eq!(exit_code(&error), exit::NEGATIVE, "{code} 属于业务否定");
        }
    }

    #[test]
    fn explain_keeps_the_code_and_message() {
        let error = ClientError::Remote {
            code: i64::from(TX_NOT_FOUND),
            message: "未知 txId：tx-404".to_string(),
        };
        let (summary, details) = explain(&error);
        let code_text = i64::from(TX_NOT_FOUND).to_string();
        assert!(summary.contains(&code_text), "结论里要带码：{summary}");
        assert!(summary.contains("tx-404"), "结论里要带 message：{summary}");
        assert!(
            details[0].contains("tx begin"),
            "第一条细节应是可执行的下一步：{:?}",
            details
        );
    }

    #[test]
    fn explain_transport_lists_all_three_fixes() {
        let error = ClientError::Transport("os error 10061".to_string());
        let (summary, details) = explain(&error);
        assert!(summary.contains("连不上引擎"), "{summary}");
        assert_eq!(details.len(), 3, "三条修法都要给全：{details:?}");
        assert!(details.iter().any(|d| d.contains("--rpc-port")));
        assert!(details.iter().any(|d| d.contains("--addr")));
        assert!(details.iter().any(|d| d.contains("--token")));
    }

    #[test]
    fn explain_unknown_code_says_so_instead_of_guessing() {
        let error = ClientError::Remote {
            code: -32024,
            message: "未登记的错误".to_string(),
        };
        let (_, details) = explain(&error);
        assert_eq!(details.len(), 1);
        assert!(details[0].contains("没有为 -32024 登记"), "{:?}", details);
    }

    #[test]
    fn nearest_methods_only_appear_for_method_not_found() {
        let near = ClientError::Remote {
            code: i64::from(METHOD_NOT_FOUND),
            message: "scene.verfy".to_string(),
        };
        let (_, details) = explain(&near);
        assert!(
            details.iter().any(|d| d.contains("最近的已知方法")),
            "拼错方法名时要给候选：{details:?}"
        );

        let other = ClientError::Remote {
            code: i64::from(TX_BUSY),
            message: "scene.verfy".to_string(),
        };
        let (_, details) = explain(&other);
        assert!(
            !details.iter().any(|d| d.contains("最近的已知方法")),
            "其他错误给候选方法等于转移注意力：{details:?}"
        );
    }
}
