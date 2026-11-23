//! 命令表：脚本命令与 RPC 方法元数据的**单一真源**（M0-S2 / S2-02）。
//!
//! 为什么落在 `r3e-core`：`xtask/dep_rules.json` 给 `r3e-script` 的 allow 只有
//! `[r3e-core]`，给 `r3e-editor-host` 的是 `[r3e-core, r3e-editor, r3e-runtime]`。
//! 命令表放 core 是两侧唯一都合法、且不新增任何依赖边的落点；放新 crate（如
//! `r3e-protocol`）会让两侧各多一条白名单外依赖，违反「dep-check 违规不许涨」。
//!
//! 本模块**只声明、不生成代码**：M0 阶段命令表是登记簿，脚本侧 `bind_all` 与宿主侧
//! `dispatch` 仍是手写的真实现，一致性由三处契约测试看守（见
//! `r3e-script/tests/cmd_spec_contract.rs`、`r3e-editor/tests/cmd_spec_contract.rs`、
//! `r3e-editor-host/tests/cmd_spec_contract.rs`）。代码生成（含 `r3e-protocol`
//! 序列化承载）留到 S5-02 从本表导出 JSON。

mod anim;
mod anim_blend;
mod audio;
mod character;
mod env;
mod input;
mod light;
mod meta;
mod render;
mod scene;
mod sim;
mod terrain;
mod ui;

/// 命令所属能力域。
///
/// 取值与 `crates/r3e-script/src/api/` 下的域文件一一对应（`basic` → [`Domain::Scene`]
/// 等），`Meta` 只容纳「只有 RPC、没有脚本函数」的命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Domain {
    /// 节点输出与生命周期、变换、材质（原 `api/basic.rs`）。
    Scene,
    /// 场景流：切换 / 回退 / 存档（原 `api/scene_flow.rs`）。
    SceneFlow,
    /// 动画播放与变形目标（原 `api/anim_morph.rs`）。
    Anim,
    /// 动画层混合树（原 `api/anim_blend.rs`）。
    AnimBlend,
    /// 动画层相位同步（原 `api/anim_sync.rs`）。
    AnimSync,
    /// 环境：相机 / 天空 / IBL（原 `api/env.rs`）。
    Env,
    /// 后处理（原 `api/postfx.rs`）。
    Render,
    /// 阴影（原 `api/shadow.rs`）。
    Shadow,
    /// UI 元素（原 `api/ui.rs`）。
    Ui,
    /// 文本布局（原 `api/text.rs`）。
    Text,
    /// 灯光（原 `api/light.rs`）。
    Light,
    /// 音频与空间化音频（原 `api/audio.rs`）。
    Audio,
    /// 物理（原 `api/physics.rs`）。
    Physics,
    /// 输入与时间（原 `api/input.rs`）。
    Input,
    /// 场景查询与 UI 事件回读（原 `api/query.rs`）。
    Query,
    /// 拾取与屏幕尺寸（原 `api/pick.rs`）。
    Pick,
    /// 地形：建块 / 采样 / 块划分 / 网格统计（M2-C06，原 `api/terrain.rs`）。
    Terrain,
    /// 只有 RPC、没有脚本函数：引擎信息 / 截图 / 脚本执行 / 只读装配。
    Meta,
}

/// 命令代价的量级，事务（M0-S4）按此决定是否拆帧或拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cost {
    /// 常数级：不遍历场景、不碰磁盘。
    Cheap,
    /// 线性：遍历节点 / 元素列表，量级随规模涨。
    Linear,
    /// 重：编译、序列化、落盘、逐帧推进或整场景重建。
    Heavy,
}

/// 本表初版统一的 `since` 标记：全部来自 M0 期的存量命令回填。
///
/// 之后新增命令应写自己的 Sprint 编号（如 `"M1-03"`），便于按版本查「这条命令哪个
/// 里程碑进来的」。
pub const SINCE_V0: &str = "M0-backfill";

/// 一条命令的元数据。
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    /// 脚本函数名（`snake_case`）；`Meta` 域条目为点分 RPC 名。
    pub name: &'static str,
    /// 对应的 RPC 方法（点分、小写、无下划线，即 `dispatch` 里 `canonical()` 之后的形态）。
    ///
    /// `None` 表示「只有脚本侧、没有对外 RPC」。
    pub rpc: Option<&'static str>,
    /// 所属能力域。
    pub domain: Domain,
    /// 参数类型序列。
    ///
    /// 重载（同名多签名，共 11 处）用 `"|"` 元素分隔各签名，其余元素是参数**类型**而非
    /// 形参名，例如 `&["f64", "&str"]`、`&["f64", "|", "&str"]`。
    /// `Meta` 域条目为空：它的参数经 JSON `params` 传递（见 `r3e-protocol`）。
    pub params: &'static [&'static str],
    /// 返回类型；重载时按注册顺序用 `/` 连接（如 `"bool/()"`）。
    pub returns: &'static str,
    /// 该命令是否「无持久副作用或可撤销」。
    ///
    /// `true` 表示只读或改动可被事务回滚；`false` 表示会改变状态（写场景、落盘、重建、
    /// 推进一帧等），事务提交前必须记账。
    pub reversible: bool,
    /// 代价量级。
    pub cost: Cost,
    /// 进入本表的里程碑。
    pub since: &'static str,
}

/// 全部命令（各域数组按固定顺序拼接）。
///
/// 用函数而不是 `pub static ALL: &[CommandSpec]`：稳定 Rust 无法在 `const` 上下文把
/// 多个 `&'static [T]` 拼成一个 static 切片，而引入拼接宏（`constcat` 等）会破坏
/// 「零新依赖」的硬约束。顺序稳定（按域文件名字母序），可放心用于快照类断言。
pub fn all() -> impl Iterator<Item = &'static CommandSpec> {
    [
        anim::ALL,
        anim_blend::ALL,
        audio::ALL,
        character::ALL,
        env::ALL,
        input::ALL,
        light::ALL,
        meta::ALL,
        render::ALL,
        scene::ALL,
        sim::ALL,
        terrain::ALL,
        ui::ALL,
    ]
    .into_iter()
    .flatten()
}

/// 按脚本函数名查（`Meta` 域的点分名也能查到）。同名的多个重载在表里已合并为一条。
pub fn by_name(name: &str) -> Option<&'static CommandSpec> {
    all().find(|spec| spec.name == name)
}

/// 按 RPC 方法名查（点分小写）。没有对外 RPC 的脚本命令查不到。
pub fn by_rpc(rpc: &str) -> Option<&'static CommandSpec> {
    all().find(|spec| spec.rpc == Some(rpc))
}

/// 遍历某域的全部命令。
pub fn by_domain(domain: Domain) -> impl Iterator<Item = &'static CommandSpec> {
    all().filter(move |spec| spec.domain == domain)
}

/// 全部脚本函数名。
///
/// 判据是「域不是 [`Domain::Meta`]」而不是 `rpc.is_none()`：有 23 条脚本命令同时有对外
/// RPC（`scene.spawn` ↔ `create_node` 之类），它们既在脚本侧可调，又能在宿主侧被调用。
pub fn script_names() -> impl Iterator<Item = &'static str> {
    all()
        .filter(|spec| spec.domain != Domain::Meta)
        .map(|spec| spec.name)
}

/// 全部 RPC 方法名（含 `Meta` 域）。
pub fn rpc_names() -> impl Iterator<Item = &'static str> {
    all().filter_map(|spec| spec.rpc)
}

/// 命令总数。
pub fn len() -> usize {
    all().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn 表非空且脚本与rpc两类都在() {
        let script = script_names().count();
        let rpc = rpc_names().count();
        let rpc_only = by_domain(Domain::Meta).count();
        assert!(script > 0, "脚本命令一条都没有，说明域模块没接上");
        assert!(rpc > 0, "RPC 方法一条都没有");
        assert_eq!(
            len(),
            script + rpc_only,
            "条目 = 脚本命令 + 仅有 RPC 的命令"
        );
        assert_eq!(
            rpc,
            rpc_only + 36,
            "76 个 RPC 方法里 36 个能对到脚本命令（含 M2-B01 的 character 六条、M2-C06 的 terrain 七条），其余 40 个只有 RPC（Meta 域：tx.* / query.* / physics.* / nav.* / material / verify / M1-S1-D 四条 / M2-C06 的 terrain.mesh）"
        );
    }

    #[test]
    fn 脚本名唯一() {
        let mut seen = HashSet::new();
        for name in script_names() {
            assert!(seen.insert(name), "脚本命令 `{name}` 在表里重复登记");
        }
    }

    #[test]
    fn rpc名唯一且是点分小写() {
        let mut seen = HashSet::new();
        for rpc in rpc_names() {
            assert!(seen.insert(rpc), "RPC 方法 `{rpc}` 在表里重复登记");
            assert!(rpc.contains('.'), "RPC 方法 `{rpc}` 应为点分名");
            assert!(
                rpc.chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c.is_ascii_digit()),
                "RPC 方法 `{rpc}` 应为小写点分名（dispatch 用 canonical() 归一后再比）"
            );
        }
    }

    #[test]
    fn meta域只出现在rpc条目上() {
        for spec in all() {
            if spec.domain == Domain::Meta {
                assert!(
                    spec.rpc.is_some(),
                    "`{}` 属于 Meta 域却不是 RPC 条目",
                    spec.name
                );
            }
        }
    }

    #[test]
    fn since都填了() {
        for spec in all() {
            assert!(!spec.since.is_empty(), "`{}` 缺 since", spec.name);
        }
    }

    #[test]
    fn 按名字和rpc都能查回来() {
        let first = all().next().expect("表为空");
        assert_eq!(by_name(first.name).map(|s| s.name), Some(first.name));
        if let Some(rpc) = first.rpc {
            assert_eq!(by_rpc(rpc).map(|s| s.name), Some(first.name));
        }
    }
}
