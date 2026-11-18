//! 出生点声明（M1-S4-B）。
//!
//! 导航子系统要回答「从玩家出生点能不能走到目标」这个问题，前提是**出生点在场景里
//! 有名字、有位置**。此前场景只有「物件」，出生点只能靠约定俗成（比如找个名叫
//! `player_start` 的节点），AI 与验证器都没法可靠引用。
//!
//! 这里给出场景文件里的独立段落 `spawns`：
//!
//! ```json
//! { "spawns": [ { "name": "start", "position": [0, 0, 0], "yaw": 0 } ] }
//! ```
//!
//! 设计取舍：
//!
//! * **不新增 `NodeKind`**：出生点是「场景元数据」，不是会渲染的物件。塞进节点图会让
//!   渲染 / 剔除 / 物理装配都多一个要跳过的种类，而它本来不属于任何一条管线；
//! * **名字是唯一主键**：同名后者覆盖前者（与 `terrain` / `materials` 同一套收敛规则），
//!   顺序即首次插入序，便于「第一个 spawn 当默认起点」这种确定性约定；
//! * **`yaw` 用弧度**：与 `Transform` 内部的旋转表示一致，场景层不再引入角度单位换算。
//!
//! 收敛逻辑集中在 [`normalize_all`]，加载路径与写入路径共用同一套，避免「写进去什么、
//! 读出来不一样」。

use serde::{Deserialize, Serialize};

/// 场景内出生点上限。超出上限的新增被丢弃（不是静默截断已有条目）。
pub const SPAWN_MAX: usize = 32;

/// 出生点名字最大字符数（按 `char` 计，不按字节）。
pub const SPAWN_NAME_MAX_LEN: usize = 64;

/// 场景文件里的一个出生点。
///
/// 字段全部有缺省值：只写 `{ "name": "start" }` 也是合法的（位置在原点、朝向零）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct SpawnDecl {
    /// 唯一主键。空白名字视为无效声明，加载时直接丢弃。
    pub name: String,
    /// 世界坐标位置（米）。非有限值逐分量归零。
    pub position: [f32; 3],
    /// 绕 Y 轴的朝向（弧度），收敛到 `[-π, π]`。
    pub yaw: f32,
}

impl Default for SpawnDecl {
    fn default() -> Self {
        Self {
            name: String::new(),
            position: [0.0, 0.0, 0.0],
            yaw: 0.0,
        }
    }
}

impl SpawnDecl {
    /// 新建一个出生点（其余字段走默认值）。
    pub fn new(name: impl Into<String>, position: [f32; 3]) -> Self {
        Self {
            name: name.into(),
            position,
            yaw: 0.0,
        }
    }

    /// 收敛到合法形态：名字去空白并截断、位置逐分量去掉 NaN / inf、yaw 归一化。
    ///
    /// 幂等（收敛两次结果相同）。
    pub fn sanitized(mut self) -> Self {
        self.name = self.name.trim().to_string();
        self.name = truncate_chars(&self.name, SPAWN_NAME_MAX_LEN);
        self.position = [
            finite_or(self.position[0], 0.0),
            finite_or(self.position[1], 0.0),
            finite_or(self.position[2], 0.0),
        ];
        self.yaw = normalize_angle(finite_or(self.yaw, 0.0));
        self
    }

    /// 名字是否有效（去空白后非空）。空名出生点无法被脚本 / 验证器引用。
    pub fn has_valid_name(&self) -> bool {
        !self.name.trim().is_empty()
    }
}

/// 整表收敛：丢空名、同名后者覆盖前者（保持首次出现的位置）、超上限截断。
///
/// 顺序语义与 `terrain` / `materials` 一致——同名覆盖是原地替换，不改变表内位置，
/// 这样「第一个 spawn」在多次写入后依然稳定。
pub fn normalize_all(raw: Vec<SpawnDecl>) -> Vec<SpawnDecl> {
    let mut out: Vec<SpawnDecl> = Vec::new();
    for spawn in raw {
        let spawn = spawn.sanitized();
        if !spawn.has_valid_name() {
            continue;
        }
        match out.iter_mut().find(|s| s.name == spawn.name) {
            Some(slot) => *slot = spawn,
            None => {
                if out.len() < SPAWN_MAX {
                    out.push(spawn);
                }
            }
        }
    }
    out
}

/// 按名字查一个出生点（表内线性查找：上限 32 条，不值得建索引）。
pub fn find<'a>(spawns: &'a [SpawnDecl], name: &str) -> Option<&'a SpawnDecl> {
    let name = name.trim();
    spawns.iter().find(|s| s.name == name)
}

fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

/// 把角度归一到 `[-π, π]`，避免「yaw 写 720°」这类输入让哈希与序列化抖动。
fn normalize_angle(mut v: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    if !v.is_finite() {
        return 0.0;
    }
    while v > PI {
        v -= TAU;
    }
    while v < -PI {
        v += TAU;
    }
    v
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

impl crate::scene::Scene {
    /// 整表替换出生点（加载路径与写入路径共用同一套收敛：丢空名、同名覆盖、超上限截断）。
    pub fn set_spawns(&mut self, spawns: Vec<SpawnDecl>) {
        self.spawns = normalize_all(spawns);
    }

    /// 新增或覆盖一个出生点；名字无效或表已满（且不是覆盖已有名字）时返回 `false`。
    ///
    /// 覆盖是原地替换（表内位置不变），这样「第一个出生点」在反复写入后依然稳定。
    pub fn upsert_spawn(&mut self, decl: SpawnDecl) -> bool {
        let decl = decl.sanitized();
        if !decl.has_valid_name() {
            return false;
        }
        match self.spawns.iter_mut().find(|s| s.name == decl.name) {
            Some(slot) => {
                *slot = decl;
                true
            }
            None => {
                if self.spawns.len() >= SPAWN_MAX {
                    return false;
                }
                self.spawns.push(decl);
                true
            }
        }
    }

    /// 按名字查出生点（名字两侧空白被忽略）。
    pub fn spawn_decl(&self, name: &str) -> Option<&SpawnDecl> {
        find(&self.spawns, name)
    }

    /// 首个出生点——导航层与验证器的默认起点。
    pub fn first_spawn(&self) -> Option<&SpawnDecl> {
        self.spawns.first()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_trims_name_and_neutralizes_non_finite() {
        let s = SpawnDecl {
            name: "  start  ".to_string(),
            position: [f32::NAN, 1.0, f32::INFINITY],
            yaw: 7.0,
        }
        .sanitized();
        assert_eq!(s.name, "start");
        assert_eq!(s.position, [0.0, 1.0, 0.0]);
        assert!(s.yaw >= -std::f32::consts::PI && s.yaw <= std::f32::consts::PI);
    }

    #[test]
    fn normalize_drops_empty_names_and_keeps_first_position() {
        let raw = vec![
            SpawnDecl::default(),
            SpawnDecl::new("a", [0.0, 0.0, 0.0]),
            SpawnDecl::new("b", [1.0, 0.0, 0.0]),
            SpawnDecl::new("a", [2.0, 0.0, 0.0]),
        ];
        let out = normalize_all(raw);
        assert_eq!(out.len(), 2);
        // 同名覆盖是原地替换：`a` 仍在首位，但内容是后写的那份。
        assert_eq!(out[0].name, "a");
        assert_eq!(out[0].position, [2.0, 0.0, 0.0]);
        assert_eq!(out[1].name, "b");
    }

    #[test]
    fn normalize_is_idempotent() {
        let raw = vec![SpawnDecl::new(" start ", [1.0, 2.0, 3.0])];
        let once = normalize_all(raw.clone());
        let twice = normalize_all(once.clone());
        assert_eq!(once, twice);
    }

    #[test]
    fn normalize_caps_table_size() {
        let raw: Vec<SpawnDecl> = (0..SPAWN_MAX + 5)
            .map(|i| SpawnDecl::new(format!("s{i}"), [i as f32, 0.0, 0.0]))
            .collect();
        assert_eq!(normalize_all(raw).len(), SPAWN_MAX);
    }

    #[test]
    fn find_is_trim_insensitive() {
        let spawns = vec![SpawnDecl::new("start", [0.0, 0.0, 0.0])];
        assert!(find(&spawns, " start ").is_some());
        assert!(find(&spawns, "other").is_none());
    }
}
