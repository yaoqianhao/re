//! 角色控制器能力面：脚本与宿主侧看到的角色接口（M2-B01）。
//!
//! 与 [`PhysicsHost`](super::physics::PhysicsHost) 同一套路：`r3e-core` 只声明
//! **诉求 / 应答 / 默认方法**，真实现落在 `r3e-character`，由装配层（运行时 /
//! 编辑器宿主）在构造 `ScriptHost` 时注入。这样 `r3e-script` 不会为了调角色而
//! 新增对 `r3e-character` 的依赖边（`xtask/dep_rules.json` 里 `r3e-script`
//! 的 allow 只有 `[core, doc, protocol]`）。
//!
//! 三条设计约束（改本文件前先读）：
//!
//! 1. **入参带 `&Node` 而不是只带名字**：配置的真值是场景里的
//!    [`Node::character`](crate::Node::character)，宿主内部只放配置的缓存副本。
//!    把 `&Node` 传进去，宿主就不会出现「场景改了配置、控制器还按旧的走」。
//! 2. **应答只用固定几种形态**：`Bool` / `State` / `Stats`，不返回
//!    `serde_json::Value`——`r3e-core` 不服侍协议层，序列化归 `r3e-protocol`。
//! 3. **默认实现永不解引用空指针也不 panic**：所有默认方法在「未接角色」
//!    时返回 `false` / `None` / 全零统计，脚本侧据此打印提示但流程不断。
//!    静默默认后端 [`NullCharacterHost`] 就是这条约束的落地形态。

use std::any::Any;

use crate::node::Node;

/// 角色控制器的运行态快照（只读、可复制、无句柄）。
///
/// 「位置」是**脚底**位置而非胶囊中心：步进结果直接拿去写 `Transform` 时不用再
/// 加减半个身高，少一类 off-by-half-height 的错。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CharacterStateDto {
    /// 脚底世界坐标。
    pub position: [f64; 3],
    /// 竖直速度（上正下负）。
    pub vertical_speed: f64,
    /// 是否站在地面上（可用于决定能不能起跳）。
    pub grounded: bool,
    /// 最近一次步进是否被阻挡（撞墙）。
    pub blocked: bool,
}

/// 角色控制器的累计统计（诊断 / 门禁用）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CharacterStatsDto {
    /// 当前登记在册的控制器数量。
    pub controllers: u64,
    /// 累计推进的步数。
    pub steps: u64,
    /// 累计被阻挡的步数（占 `steps` 的一部分）。
    pub blocked: u64,
    /// 累计跨过的台阶次数。
    pub step_ups: u64,
}

/// 脚本对角色控制器的全部诉求。实现方只需给出 `execute`。
#[derive(Debug)]
pub enum CharReq<'a> {
    /// 按 `node` 里的配置建立或刷新控制器（幂等：同名重复调用只刷新配置）。
    Bind {
        /// 携带 [`crate::CharacterSpec`] 的节点。
        node: &'a Node,
    },
    /// 丢弃某个控制器的运行态。
    Unbind {
        /// 节点名。
        name: &'a str,
    },
    /// 推进一个固定步长。
    ///
    /// `wish` 是**水平**意图方向（世界 XY 对应引擎 XZ），长度会被归一化后当作
    /// 本步的完整步长处理；`jump` 只在着地时生效；`dt` 由调用方给（固定 1/60 是
    /// 运行时的口径，测试可自由缩短）。
    Step {
        /// 携带配置的节点（同时给出当前脚底位置与朝向）。
        node: &'a Node,
        /// 水平意图方向 `[x, z]`，长度 0 表示原地不动。
        wish: [f64; 2],
        /// 本步是否请求起跳。
        jump: bool,
        /// 步长（秒）。
        dt: f64,
    },
    /// 读取运行态。控制器不存在返回 `None`。
    State {
        /// 节点名。
        name: &'a str,
    },
    /// 直接落位（把竖直速度清零，用于传送/复活）。
    Teleport {
        /// 节点名。
        name: &'a str,
        /// 目标脚底位置。
        position: [f64; 3],
    },
    /// 读累计统计。
    Stats,
    /// 读后端名（用于诊断「到底接没接真实现」）。
    Backend,
}

/// 诉求的应答。
#[derive(Debug, Clone, PartialEq)]
pub enum CharRsp {
    /// 成功 / 失败。
    Bool(bool),
    /// 运行态；`None` = 没有这个控制器。
    State(Option<CharacterStateDto>),
    /// 统计。
    Stats(CharacterStatsDto),
    /// 文本（后端名）。
    Text(String),
}

/// 脚本与宿主侧看到的角色能力面。
///
/// 实现方只需要写 `execute` 与 `as_any`；下面全是默认方法，脚本 API 层与
/// 编辑器宿主统一走这些形态。
pub trait CharacterHost {
    /// 函数式适配器入口：`r3e-character` 用它把真实控制器接进来。
    fn execute(&self, req: CharReq<'_>) -> CharRsp;

    /// 让装配层能从 trait 背后取回自己的实现体（`as_any().downcast_ref()`）。
    fn as_any(&self) -> &dyn Any;

    /// 按节点的 `character` 配置建立/刷新控制器；节点没配置返回 `false`。
    fn bind(&self, node: &Node) -> bool {
        matches!(self.execute(CharReq::Bind { node }), CharRsp::Bool(true))
    }

    /// 丢弃控制器；不存在返回 `false`。
    fn unbind(&self, name: &str) -> bool {
        matches!(self.execute(CharReq::Unbind { name }), CharRsp::Bool(true))
    }

    /// 推进一步；控制器不存在返回 `None`（脚本侧据此提示「先 create」）。
    fn step(&self, node: &Node, wish: [f64; 2], jump: bool, dt: f64) -> Option<CharacterStateDto> {
        match self.execute(CharReq::Step {
            node,
            wish,
            jump,
            dt,
        }) {
            CharRsp::State(s) => s,
            _ => None,
        }
    }

    /// 读运行态。
    fn state(&self, name: &str) -> Option<CharacterStateDto> {
        match self.execute(CharReq::State { name }) {
            CharRsp::State(s) => s,
            _ => None,
        }
    }

    /// 直接落位；控制器不存在返回 `false`。
    fn teleport(&self, name: &str, position: [f64; 3]) -> bool {
        matches!(
            self.execute(CharReq::Teleport { name, position }),
            CharRsp::Bool(true)
        )
    }

    /// 累计统计。
    fn stats(&self) -> CharacterStatsDto {
        match self.execute(CharReq::Stats) {
            CharRsp::Stats(s) => s,
            _ => CharacterStatsDto::default(),
        }
    }

    /// 后端名；未接真实实现时为 `"none"`。
    fn backend(&self) -> String {
        match self.execute(CharReq::Backend) {
            CharRsp::Text(t) => t,
            _ => "none".to_string(),
        }
    }
}

/// 空实现：一切诉求都回「没有」，不报错、不 panic。
///
/// 用于三类场景：单测 / 编辑器不装配角色 / `r3e-script` 的三个便利构造函数
/// （`ScriptHost::new` 等不显式给角色的入口）。真装配走
/// `ScriptHost::new_with_character`。
#[derive(Debug, Default, Clone, Copy)]
pub struct NullCharacterHost;

impl NullCharacterHost {
    /// 构造。
    pub const fn new() -> Self {
        Self
    }
}

impl CharacterHost for NullCharacterHost {
    fn execute(&self, req: CharReq<'_>) -> CharRsp {
        match req {
            CharReq::State { .. } => CharRsp::State(None),
            CharReq::Stats => CharRsp::Stats(CharacterStatsDto::default()),
            CharReq::Backend => CharRsp::Text("none".to_string()),
            CharReq::Bind { .. } | CharReq::Unbind { .. } | CharReq::Step { .. } => {
                CharRsp::Bool(false)
            }
            CharReq::Teleport { .. } => CharRsp::Bool(false),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 闭包即适配器：`F: Fn(CharReq) -> CharRsp` 直接就是一个角色后端。
///
/// 与 [`PhysicsHost`](super::physics::PhysicsHost) / 音频同一约定，测试里写一个
/// 闭包就是一套后端，不必为每个用例建结构体。
impl<F> CharacterHost for F
where
    F: for<'a> Fn(CharReq<'a>) -> CharRsp + 'static,
{
    fn execute(&self, req: CharReq<'_>) -> CharRsp {
        self(req)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node_with_spec() -> Node {
        let mut n = Node::new(1, "hero");
        n.character = Some(crate::CharacterSpec::default());
        n
    }

    #[test]
    fn 空实现一律回否且不panic() {
        let host = NullCharacterHost::new();
        let node = node_with_spec();
        assert!(!host.bind(&node));
        assert!(!host.unbind("hero"));
        assert!(host.step(&node, [1.0, 0.0], false, 1.0 / 60.0).is_none());
        assert!(host.state("hero").is_none());
        assert!(!host.teleport("hero", [0.0, 0.0, 0.0]));
        assert_eq!(host.stats(), CharacterStatsDto::default());
        assert_eq!(host.backend(), "none");
    }

    #[test]
    fn 空实现可以downcast() {
        let host = NullCharacterHost::new();
        assert!(host.as_any().downcast_ref::<NullCharacterHost>().is_some());
    }
}
