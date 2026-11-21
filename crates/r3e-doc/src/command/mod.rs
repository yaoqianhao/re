//! 命令层 v0：把「改场景」变成可命名、可撤销的值。
//!
//! 一条命令 = 一个自包含的值对象（[`CommandKind`]），既知道怎么 `apply`，也知道怎么
//! `undo`。`undo` 靠的是**前像**：构造命令时从当前场景读出旧值一并带走，撤销就是把
//! 旧值写回去，不依赖任何全局快照。
//!
//! 与 `SceneCommand`（`r3e-core::scene_stack`）的分工：那是**整个场景**的存档/回放单元
//! （场景栈换档用），这里是**单个编辑动作**的粒度（编辑器撤销、脚本可审计、S4 差分哈希）。
//!
//! v0 落地 17 条，按域分组：
//!
//! | 域 | 命令 |
//! |---|---|
//! | 节点 | `SpawnNode` `SetTransform` `SetVisible` `SetNodeColor` `SetNodeMaterial` |
//! | 环境 | `SetCamera` `SetEnvironment` `SetPostFx` |
//! | 灯光 | `UpsertLocalLight` `RemoveLocalLight` |
//! | 地形 | `UpsertTerrainPatch` |
//! | 材质 | `UpsertMaterial` `RemoveMaterial` `SetNodeMaterialRef` |
//! | UI / 文本 | `UpsertUi` `RemoveUi` `UpsertText` `RemoveText` |
//!
//! 未纳入（M1）：`DespawnNode` / `RenameNode` / `SetAnim*` / `SetMorph*`。前者需要 core
//! 侧补「删除前像 → 重新插入」的 API（`Node` 无法由公开接口再造），后三者要等命令面
//! 覆盖动画与形变域，见 `docs/m0/M0S3_DOC_LAYER.md` §7 偏离登记。

mod env;
mod material;
mod scene;
mod terrain;
mod ui;

use r3e_core::{
    Camera, CharacterSpec, EnvironmentSettings, LocalLight, NodeId, ObjectDesc, PostFxSettings,
    Scene, SceneMaterialDecl, ScreenText, SurfaceMaterial, TerrainPatch, Transform, UiElement,
    Vec4,
};

use crate::{DocError, Document, Mark, UndoGroup};

/// 可撤销命令。
///
/// 实现方只需给出 [`Command::kind`]，`apply` / `undo` 默认转交给值对象。
/// 自定义命令（上层想加自己的编辑动作）实现这个 trait 即可进 [`Document::apply`]，
/// 不需要改 `r3e-doc`。
pub trait Command {
    /// 命令的值形态（自带前像，可直接入撤销栈）。
    fn kind(&self) -> CommandKind;

    /// 执行。失败时不留下记账痕迹（见 [`Document::apply`]）。
    fn apply(&self, scene: &mut Scene) -> Result<(), DocError> {
        self.kind().apply(scene)
    }

    /// 撤销。
    fn undo(&self, scene: &mut Scene) -> Result<(), DocError> {
        self.kind().undo(scene)
    }

    /// 展示名（撤销栈 UI、日志）。
    fn label(&self) -> &'static str {
        self.kind().label()
    }
}

/// 命令值对象：新值 + 前像。
///
/// 前像在**构造时**从场景抓取，因此命令一旦造出来就与场景解耦：先造一批、稍后统一
/// 执行、失败再整体回滚，都是安全的。
/// 注意：`ObjectDesc` / `Camera` 在 core 里没有 `PartialEq`，所以这里不推导 `PartialEq`；
/// 命令相等性请按变体 + 前像字段自行比对（测试里用 `matches!`）。
#[derive(Debug, Clone)]
pub enum CommandKind {
    /// 新建物件。撤销按名字删除（`spawn` 重名即失败，名字唯一，删除精确）。
    SpawnNode {
        /// 物件描述（撤销时用其中的 `name` 定位）。
        desc: ObjectDesc,
    },
    /// 改节点变换。
    SetTransform {
        /// 目标节点。
        id: NodeId,
        /// 前像。
        old: Transform,
        /// 新值。
        new: Transform,
    },
    /// 改节点可见性。
    SetVisible {
        /// 目标节点。
        id: NodeId,
        /// 前像。
        old: bool,
        /// 新值。
        new: bool,
    },
    /// 改节点颜色（RGBA 线性）。
    SetNodeColor {
        /// 目标节点。
        id: NodeId,
        /// 前像。
        old: Vec4,
        /// 新值。
        new: Vec4,
    },
    /// 整体写物件材质；`None` = 清掉覆盖、回退到网格默认材质。
    SetNodeMaterial {
        /// 目标节点名。
        name: String,
        /// 前像。
        old: Option<SurfaceMaterial>,
        /// 新值。
        new: Option<SurfaceMaterial>,
    },
    /// 换摄像机。
    SetCamera {
        /// 前像。
        old: Camera,
        /// 新值。
        new: Camera,
    },
    /// 换环境探针（`None` = 回退常数环境光）。
    SetEnvironment {
        /// 前像。
        old: Option<EnvironmentSettings>,
        /// 新值。
        new: Option<EnvironmentSettings>,
    },
    /// 换 HDR 后处理参数。
    SetPostFx {
        /// 前像。
        old: Option<PostFxSettings>,
        /// 新值。
        new: Option<PostFxSettings>,
    },
    /// 按 key 写局部光源（同 key 覆盖）。
    UpsertLocalLight {
        /// 新光源。
        next: LocalLight,
        /// 前像；`None` 表示此前没有这盏灯，撤销即删除。
        old: Option<LocalLight>,
    },
    /// 按 key 删局部光源。
    RemoveLocalLight {
        /// 被删的光源（撤销即原样写回）。
        old: LocalLight,
    },
    /// 按 key 写 UI 元素。
    UpsertUi {
        /// 新元素。
        next: UiElement,
        /// 前像。
        old: Option<UiElement>,
    },
    /// 按 key 删 UI 元素。
    RemoveUi {
        /// 被删元素。
        old: UiElement,
    },
    /// 按 key 写屏幕文本。
    UpsertText {
        /// 新文本。
        next: ScreenText,
        /// 前像。
        old: Option<ScreenText>,
    },
    /// 按 key 删屏幕文本。
    RemoveText {
        /// 被删文本。
        old: ScreenText,
    },
    /// 按 name 写地形块（同名覆盖，M1-S3-C）。
    UpsertTerrainPatch {
        /// 新地形块。
        next: TerrainPatch,
        /// 前像；`None` 表示此前没有这块地形，撤销即删除。
        old: Option<TerrainPatch>,
    },
    /// 按 name 写场景材质（同名覆盖，M1-S4-A）。
    UpsertMaterial {
        /// 新材质。
        next: SceneMaterialDecl,
        /// 前像；`None` 表示此前没有这份材质，撤销即删除。
        old: Option<SceneMaterialDecl>,
    },
    /// 按 name 删场景材质（M1-S4-A）。
    RemoveMaterial {
        /// 被删的材质（撤销即原样写回）。
        old: SceneMaterialDecl,
    },
    /// 改节点的场景材质引用（M1-S4-A）；`None` = 清除引用。
    SetNodeMaterialRef {
        /// 目标节点名。
        name: String,
        /// 前像（引用名本身，不是解析后的材质）。
        old: Option<String>,
        /// 新值。
        new: Option<String>,
    },
    /// 改节点的角色控制器配置（M2-B01）；`None` = 该节点不再是角色。
    ///
    /// `CharacterSpec` 只是**静态配置**，所以进撤销栈是安全的：运行态
    /// （位置 / 速度 / 是否着地）一律不进场景，否则「同一场景两次回放」
    /// 会因为角色走到哪儿而产生不同的 `state_hash`。
    SetNodeCharacter {
        /// 目标节点名。
        name: String,
        /// 前像。
        old: Option<CharacterSpec>,
        /// 新值。
        new: Option<CharacterSpec>,
    },
}

impl CommandKind {
    /// 展示名（中文短标签，撤销栈直接显示）。
    pub fn label(&self) -> &'static str {
        match self {
            Self::SpawnNode { .. } => "新建物件",
            Self::SetTransform { .. } => "移动/旋转/缩放",
            Self::SetVisible { .. } => "切换可见性",
            Self::SetNodeColor { .. } => "改颜色",
            Self::SetNodeMaterial { .. } => "改材质",
            Self::SetCamera { .. } => "改摄像机",
            Self::SetEnvironment { .. } => "改环境",
            Self::SetPostFx { .. } => "改后处理",
            Self::UpsertLocalLight { .. } => "写局部光",
            Self::RemoveLocalLight { .. } => "删局部光",
            Self::UpsertUi { .. } => "写 UI",
            Self::RemoveUi { .. } => "删 UI",
            Self::UpsertText { .. } => "写文本",
            Self::RemoveText { .. } => "删文本",
            Self::UpsertTerrainPatch { .. } => "写地形",
            Self::UpsertMaterial { .. } => "写材质",
            Self::RemoveMaterial { .. } => "删材质",
            Self::SetNodeMaterialRef { .. } => "改材质引用",
            Self::SetNodeCharacter { .. } => "改角色配置",
        }
    }

    /// 执行：把 `new` 写进场景。
    pub fn apply(&self, s: &mut Scene) -> Result<(), DocError> {
        match self {
            Self::SpawnNode { desc } => scene::spawn(s, desc),
            Self::SetTransform { id, new, .. } => scene::write_transform(s, *id, *new),
            Self::SetVisible { id, new, .. } => scene::write_visible(s, *id, *new),
            Self::SetNodeColor { id, new, .. } => scene::write_color(s, *id, *new),
            Self::SetNodeMaterial { name, new, .. } => scene::write_material(s, name, new),
            Self::SetCamera { new, .. } => env::write_camera(s, new),
            Self::SetEnvironment { new, .. } => env::write_environment(s, new),
            Self::SetPostFx { new, .. } => env::write_postfx(s, new),
            Self::UpsertLocalLight { next, .. } => env::write_local_light(s, next),
            Self::RemoveLocalLight { old } => env::remove_local_light(s, &old.key),
            Self::UpsertUi { next, .. } => ui::write_ui(s, next),
            Self::RemoveUi { old } => ui::remove_ui(s, &old.key),
            Self::UpsertText { next, .. } => ui::write_text(s, next),
            Self::RemoveText { old } => ui::remove_text(s, &old.key),
            Self::UpsertTerrainPatch { next, .. } => terrain::write_patch(s, next),
            Self::UpsertMaterial { next, .. } => material::write_material(s, next),
            Self::RemoveMaterial { old } => material::remove_material(s, &old.name),
            Self::SetNodeMaterialRef { name, new, .. } => {
                material::write_node_material_ref(s, name, new)
            }
            Self::SetNodeCharacter { name, new, .. } => scene::write_character(s, name, new),
        }
    }

    /// 撤销：把前像写回去。
    pub fn undo(&self, s: &mut Scene) -> Result<(), DocError> {
        match self {
            Self::SpawnNode { desc } => scene::despawn(s, &desc.name),
            Self::SetTransform { id, old, .. } => scene::write_transform(s, *id, *old),
            Self::SetVisible { id, old, .. } => scene::write_visible(s, *id, *old),
            Self::SetNodeColor { id, old, .. } => scene::write_color(s, *id, *old),
            Self::SetNodeMaterial { name, old, .. } => scene::write_material(s, name, old),
            Self::SetCamera { old, .. } => env::write_camera(s, old),
            Self::SetEnvironment { old, .. } => env::write_environment(s, old),
            Self::SetPostFx { old, .. } => env::write_postfx(s, old),
            Self::UpsertLocalLight { next, old } => match old {
                Some(old) => env::write_local_light(s, old),
                None => env::remove_local_light(s, &next.key),
            },
            Self::RemoveLocalLight { old } => env::write_local_light(s, old),
            Self::UpsertUi { next, old } => match old {
                Some(old) => ui::write_ui(s, old),
                None => ui::remove_ui(s, &next.key),
            },
            Self::RemoveUi { old } => ui::write_ui(s, old),
            Self::UpsertText { next, old } => match old {
                Some(old) => ui::write_text(s, old),
                None => ui::remove_text(s, &next.key),
            },
            Self::RemoveText { old } => ui::write_text(s, old),
            Self::UpsertTerrainPatch { next, old } => match old {
                Some(old) => terrain::write_patch(s, old),
                None => terrain::remove_patch(s, &next.name),
            },
            Self::UpsertMaterial { next, old } => match old {
                Some(old) => material::write_material(s, old),
                None => material::remove_material(s, &next.name),
            },
            Self::RemoveMaterial { old } => material::write_material(s, old),
            Self::SetNodeMaterialRef { name, old, .. } => {
                material::write_node_material_ref(s, name, old)
            }
            Self::SetNodeCharacter { name, old, .. } => scene::write_character(s, name, old),
        }
    }

    /// 命令触及的节点名（若按名字寻址），诊断与冲突检测用。
    pub fn target_name(&self) -> Option<&str> {
        match self {
            Self::SpawnNode { desc } => Some(&desc.name),
            Self::SetNodeMaterial { name, .. } => Some(name),
            Self::UpsertLocalLight { next, .. } => Some(&next.key),
            Self::RemoveLocalLight { old } => Some(&old.key),
            Self::UpsertUi { next, .. } => Some(&next.key),
            Self::RemoveUi { old } => Some(&old.key),
            Self::UpsertText { next, .. } => Some(&next.key),
            Self::RemoveText { old } => Some(&old.key),
            Self::UpsertTerrainPatch { next, .. } => Some(&next.name),
            Self::UpsertMaterial { next, .. } => Some(&next.name),
            Self::RemoveMaterial { old } => Some(&old.name),
            Self::SetNodeMaterialRef { name, .. } => Some(name),
            _ => None,
        }
    }
}

impl Command for CommandKind {
    fn kind(&self) -> CommandKind {
        self.clone()
    }

    fn apply(&self, scene: &mut Scene) -> Result<(), DocError> {
        CommandKind::apply(self, scene)
    }

    fn undo(&self, scene: &mut Scene) -> Result<(), DocError> {
        CommandKind::undo(self, scene)
    }

    fn label(&self) -> &'static str {
        CommandKind::label(self)
    }
}

impl Document {
    /// 执行命令：成功才记账（版本 +1、置脏、写次数 +1），失败连版本都不动。
    ///
    /// 失败不推进版本很重要：撤销栈与版本号必须严格同步，否则 S4 的差分哈希会漏掉
    /// 一次「尝试了但没改」的空转。
    pub fn apply(&mut self, cmd: &dyn Command) -> Result<(), DocError> {
        let kind = cmd.kind();
        kind.apply(&mut self.scene)?;
        self.touch();
        self.undo.push_single(kind);
        Ok(())
    }

    /// 撤销最近**一笔**历史。栈空返回 [`DocError::NothingToUndo`]。
    ///
    /// 「一笔」= 一次 `apply`（单命令）或一次事务 `commit`（n 条命令）。因此一次 undo
    /// 会把整笔事务原样退回去，而不是退一条。
    ///
    /// 失败时该笔里没撤掉的命令留在栈上（只有成功撤销才出栈），避免「撤销失败还丢了历史」。
    pub fn undo(&mut self) -> Result<(), DocError> {
        let mark = self
            .undo
            .last()
            .map(UndoGroup::mark)
            .ok_or(DocError::NothingToUndo)?;
        self.undo.rollback_to(mark, &mut self.scene)?;
        self.touch();
        Ok(())
    }

    /// 回滚到水位 `mark`：撤销该标记及之后的**所有笔**（逆序），返回撤销的命令条数。
    ///
    /// 事务用它做原子回退边界；上层也能用它做「退回某个快照点」。
    pub fn rollback_to(&mut self, mark: Mark) -> Result<usize, DocError> {
        let undone = self.undo.rollback_to(mark, &mut self.scene)?;
        if undone > 0 {
            self.invalidate_hash_cache();
            self.touch();
        }
        Ok(undone)
    }

    /// 撤销栈深度（**笔数**，不是命令条数）。
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// 撤销栈里累计的命令条数（含事务内的 n 条）。
    pub fn undo_commands(&self) -> usize {
        self.undo.total_cmds()
    }

    /// 下一次事务的水位标记（`rollback_to` 的边界锚点）。
    pub fn undo_mark(&self) -> Mark {
        self.undo.next_mark()
    }

    /// 清空撤销栈（存盘 / 换文档后调用）。
    pub fn clear_undo(&mut self) {
        self.undo.clear();
    }

    /// 最近一条命令的展示名（没有则为 `None`）。
    pub fn undo_label(&self) -> Option<&'static str> {
        self.undo.last_label()
    }
}
