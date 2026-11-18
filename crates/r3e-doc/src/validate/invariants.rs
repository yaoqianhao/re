//! 不变量登记表（M0-S5 / S5-02）：校验器能报出的**全部** `Issue::code` 的唯一真源。
//!
//! 为什么单独列表：`code` 是给机器读的稳定标识（测试断言、编辑器筛选、对外能力清单
//! `docs/capabilities.json` 都按它索引），散落在 `bounds.rs` / `refs.rs` 的字符串字面量里
//! 无法枚举、无法比对。本表把「引擎声明自己会报哪些问题」变成可被代码读取的数据，
//! 于是三件事都成了集合运算：清单生成、清单漂移检测（`xtask invariants`）、
//! 反例测试覆盖率。
//!
//! **维护规则**：新增校验规则时，先在本表登记，再在 `bounds.rs` / `refs.rs` 里用同一
//! 字面量上报。两处不一致由 `xtask invariants` 与 `issue_codes_contract` 测试同时看守。
//! 改 `code` 文本等于破坏对外契约，需 `schema++` 并同步消费方（ai_ide）。

use super::Severity;

/// 一条不变量的元数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssueCode {
    /// 机器可读标识，点分小写（`node.name.empty`）。
    pub code: &'static str,
    /// 上报该 code 的校验层（对应 `validate/` 下的模块名）。
    pub layer: &'static str,
    /// 严重度：`Error` 表示场景自相矛盾，`Warn` 表示可疑但可用。
    pub severity: Severity,
}

/// 全部不变量，按校验层分组、组内按上报顺序排列（与源码阅读顺序一致）。
///
/// 顺序本身无契约含义；对外清单会按 `code` 字典序重排（见 `r3e_protocol::caps`）。
pub const ISSUE_CODES: &[IssueCode] = &[
    // ---------------------------------------------------------------- bounds.rs
    IssueCode {
        code: "lights.over_limit",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "node.name.empty",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "node.name.duplicate",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "node.transform.non_finite",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "node.color.non_finite",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "node.material.non_finite",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "light.key.empty",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "light.key.duplicate",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "light.value.non_finite",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "ui.key.empty",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "ui.key.duplicate",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "text.key.empty",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "text.key.duplicate",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "morph.target_count.over_max",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "morph.weight.non_finite",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "scene.background.non_finite",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "camera.non_finite",
        layer: "bounds",
        severity: Severity::Error,
    },
    IssueCode {
        code: "physics.gravity.non_finite",
        layer: "bounds",
        severity: Severity::Error,
    },
    // ---------------------------------------------------------------- refs.rs
    IssueCode {
        code: "ref.node_missing",
        layer: "refs",
        severity: Severity::Error,
    },
    IssueCode {
        code: "anim.name.empty",
        layer: "refs",
        severity: Severity::Error,
    },
    IssueCode {
        code: "skin.name.empty",
        layer: "refs",
        severity: Severity::Error,
    },
    IssueCode {
        code: "skin.joint_matrices.unsolved",
        layer: "refs",
        severity: Severity::Warn,
    },
    IssueCode {
        code: "skin.joint_matrices.len",
        layer: "refs",
        severity: Severity::Error,
    },
    IssueCode {
        code: "ref.asset_missing",
        layer: "refs",
        severity: Severity::Error,
    },
    // ---------------------------------------------------------------- material.rs
    IssueCode {
        code: "material.dup_name",
        layer: "material",
        severity: Severity::Error,
    },
    IssueCode {
        code: "material.params_out_of_range",
        layer: "material",
        severity: Severity::Error,
    },
    IssueCode {
        code: "material.texture_missing",
        layer: "material",
        severity: Severity::Error,
    },
    IssueCode {
        code: "material.ref_missing",
        layer: "material",
        severity: Severity::Error,
    },
    IssueCode {
        code: "material.ref_conflict",
        layer: "material",
        severity: Severity::Error,
    },
    IssueCode {
        code: "material.unreferenced",
        layer: "material",
        severity: Severity::Warn,
    },
    // ------------------------------------------------------- overlap.rs（静态近似）
    IssueCode {
        code: "overlap.suspect",
        layer: "overlap",
        severity: Severity::Warn,
    },
    // ------------------------------------------------------- support.rs（静态近似）
    IssueCode {
        code: "support.suspect",
        layer: "support",
        severity: Severity::Warn,
    },
    // -------------------------------------------- physics（r3e-editor-host/verify）
    IssueCode {
        code: "overlap.penetration",
        layer: "physics",
        severity: Severity::Error,
    },
    IssueCode {
        code: "support.floating",
        layer: "physics",
        severity: Severity::Warn,
    },
    // ------------------------------------------------ nav（r3e-editor-host/verify）
    IssueCode {
        code: "nav.spawn_offmesh",
        layer: "nav",
        severity: Severity::Error,
    },
    IssueCode {
        code: "nav.target_offmesh",
        layer: "nav",
        severity: Severity::Error,
    },
    IssueCode {
        code: "nav.no_path",
        layer: "nav",
        severity: Severity::Error,
    },
    IssueCode {
        code: "nav.region_split",
        layer: "nav",
        severity: Severity::Warn,
    },
];

/// 按 `code` 查表。查不到返回 `None`（代表上报了一个没登记的字面量，属于缺陷）。
pub fn by_code(code: &str) -> Option<&'static IssueCode> {
    ISSUE_CODES.iter().find(|ic| ic.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn 表内code唯一且非空() {
        let mut seen = BTreeSet::new();
        for ic in ISSUE_CODES {
            assert!(!ic.code.is_empty(), "code 不许为空");
            assert!(
                ic.code.contains('.'),
                "code 必须是点分小写形态：{}",
                ic.code
            );
            assert!(
                ic.code
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_'),
                "code 只许小写字母 / 数字 / 下划线 / 点：{}",
                ic.code
            );
            assert!(seen.insert(ic.code), "code 重复：{}", ic.code);
        }
    }

    #[test]
    fn 层名只有校验模块名() {
        for ic in ISSUE_CODES {
            assert!(
                matches!(
                    ic.layer,
                    "bounds" | "refs" | "material" | "overlap" | "support" | "physics" | "nav"
                ),
                "未知校验层 {}（新增校验模块须同步本表）",
                ic.layer
            );
        }
    }

    #[test]
    fn by_code能查回每一条() {
        for ic in ISSUE_CODES {
            assert_eq!(by_code(ic.code).map(|x| x.code), Some(ic.code));
        }
        assert!(by_code("no.such.code").is_none());
    }

    #[test]
    fn 只有蒙皮未求解与未引用材质是warn其余是error() {
        for ic in ISSUE_CODES {
            let expect_warn = matches!(
                ic.code,
                "skin.joint_matrices.unsolved"
                    | "material.unreferenced"
                    // 静态层两条是 AABB 近似初筛，误报成本高于漏报成本，一律 Warn
                    | "overlap.suspect"
                    | "support.suspect"
                    // 悬空可以是合法设计（浮空平台），物理层确认后仍只提示
                    | "support.floating"
                    // 连通块分裂本身不等于关卡走不通（多区域内传送是合法玩法）
                    | "nav.region_split"
            );
            assert_eq!(
                ic.severity == Severity::Warn,
                expect_warn,
                "严重度与校验器实现不符：{}",
                ic.code
            );
        }
    }
}
