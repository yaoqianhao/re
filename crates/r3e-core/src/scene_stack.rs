//! 场景栈与运行时快照（扩展十八 B）。
//!
//! 引擎的既有约定是「一个进程一个场景」：启动时读一份场景文件，之后靠改这份场景
//! （编辑器 RPC、脚本、热重载）往前走。要做「主菜单 → 关卡 → 暂停设置 → 回到关卡」
//! 这类流程，缺的不是"再解析一份 JSON"——那是 `Scene::from_json_str` 已经会做的事——
//! 缺的是**三件配套的东西**：
//!
//! 1. **来时的路**：切走之后还得能切回来，且切回来时不是"从头开始"，而是
//!    "接着刚才那一局"。所以栈里除了场景源文本，还要存一份运行时快照。
//! 2. **运行时的账**：`Scene::to_scene_file` 刻意只写源数据（不写 `time` / `frame` /
//!    动画播放进度 / 通道开关），这样「保存 → 重开 → 画面一致」才成立。但存档要的
//!    恰恰是那些被刻意省略的东西，所以它们单独成一节 [`RuntimeSnapshot`]。
//! 3. **边界**：本文档配套的 `docs/ARCHITECTURE.md` §11.1 把"一个进程一个场景"写成
//!    明确边界——同一时刻只有一个场景在跑，切换是**替换**而非并存，[`SceneStack`]
//!    存的是"离开时的样子"，不是"同时在跑的世界"。
//!
//! 本模块只做纯数据与纯函数：不碰文件系统以外的一切（`save_to_path` / `load_path`
//! 是唯一 IO），不认识渲染器、不认识脚本宿主，因此可以被 `r3e-app` 的宿主实现与
//! `r3e-script` 的绑定同时复用。

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::scene::{Scene, SceneFile};

/// 场景栈深度上限。
///
/// 16 层远超"菜单 / 关卡 / 设置 / 过场"这类真实游戏流程的需要，同时给内存一个
/// 硬上限：每层都握着完整场景源文本，无上限的栈在脚本里 `while true` 地压栈会
/// 直接把进程吃光。超限时丢**最老**的一层（见 [`SceneStack::push`]）。
pub const MAX_SCENE_STACK: usize = 16;

/// 运行时快照的格式版本。
///
/// 只在字段语义发生变化时递增；新增带 `#[serde(default)]` 的字段不算破坏性变更。
pub const SNAPSHOT_VERSION: u32 = 1;

/// 存档格式标识（写进 JSON 的 `format` 字段，便于人工识别与排错）。
pub const ARCHIVE_FORMAT: &str = "r3e.scene-archive";

/// 存档格式版本。
pub const ARCHIVE_VERSION: u32 = 1;

/// 单个动画实例的运行时状态。
///
/// 只记"这一局播到哪了"：片段由场景源数据决定（`anim` 段 / 资产声明），这里不重复。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimRuntime {
    /// 实例名（与 `Scene::anim_instance` 的键一致）。
    pub name: String,
    /// 播放本地时间（秒，未取模）。
    #[serde(default)]
    pub time: f32,
    /// 播放速度。
    #[serde(default = "one")]
    pub speed: f32,
    /// 是否在播。
    #[serde(default = "yes")]
    pub playing: bool,
}

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

fn snapshot_version() -> u32 {
    SNAPSHOT_VERSION
}

fn archive_version() -> u32 {
    ARCHIVE_VERSION
}

fn archive_format() -> String {
    ARCHIVE_FORMAT.to_string()
}

/// 运行时快照：`Scene::to_scene_file` **刻意不写**的那部分状态。
///
/// 设计口径与"保存"一致地板正：只收录**会随一局游戏变化、又不属于场景声明**的量。
/// 因此这里没有相机 / 灯光 / 物件——它们在场景源里，读档时由 `SceneFile` 负责；
/// 也没有 `time` / `frame` 之外的派生量（世界矩阵、蒙皮关节矩阵每帧重算，不存）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    /// 快照格式版本。
    #[serde(default = "snapshot_version")]
    pub version: u32,
    /// 场景本地时间（秒）。
    #[serde(default)]
    pub time: f32,
    /// 帧序号。
    #[serde(default)]
    pub frame: u64,
    /// 2D 文本通道开关。
    #[serde(default = "yes")]
    pub text_enabled: bool,
    /// 2D UI 通道开关。
    #[serde(default = "yes")]
    pub ui_enabled: bool,
    /// 局部光源开关。
    #[serde(default = "yes")]
    pub lights_enabled: bool,
    /// UI 与文本的前后关系。
    #[serde(default)]
    pub ui_above_text: bool,
    /// 动画实例播放状态，按实例名排序（序列化结果稳定，便于 diff）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anims: Vec<AnimRuntime>,
}

impl Default for RuntimeSnapshot {
    /// 缺省值与「刚 `Scene::new()` 出来的场景」一致：通道全开、时间归零。
    fn default() -> Self {
        Self {
            version: SNAPSHOT_VERSION,
            time: 0.0,
            frame: 0,
            text_enabled: true,
            ui_enabled: true,
            lights_enabled: true,
            ui_above_text: false,
            anims: Vec::new(),
        }
    }
}

impl RuntimeSnapshot {
    /// 从场景里抓一份快照。
    pub fn capture(scene: &Scene) -> Self {
        let mut anims: Vec<AnimRuntime> = scene
            .animations
            .iter()
            .map(|a| AnimRuntime {
                name: a.name.clone(),
                time: a.time,
                speed: a.speed,
                playing: a.playing,
            })
            .collect();
        anims.sort_by(|a, b| a.name.cmp(&b.name));
        Self {
            version: SNAPSHOT_VERSION,
            time: scene.time,
            frame: scene.frame,
            text_enabled: scene.text_enabled,
            ui_enabled: scene.ui_enabled,
            lights_enabled: scene.lights_enabled,
            ui_above_text: scene.ui_above_text,
            anims,
        }
    }

    /// 版本是否被当前引擎接受。
    pub fn is_compatible(&self) -> bool {
        self.version <= SNAPSHOT_VERSION
    }

    /// 把快照写回场景；返回**成功恢复**的动画实例数。
    ///
    /// 实例名匹配不上时静默跳过而不是报错：场景源可能在新版本里换了资产名，
    /// 这时"少恢复一个动画"比"整份存档读不进来"合理得多。计数值交给调用方决定怎么说。
    pub fn apply(&self, scene: &mut Scene) -> usize {
        scene.time = self.time;
        scene.frame = self.frame;
        scene.text_enabled = self.text_enabled;
        scene.ui_enabled = self.ui_enabled;
        scene.lights_enabled = self.lights_enabled;
        scene.ui_above_text = self.ui_above_text;

        let mut restored = 0;
        for want in &self.anims {
            if let Some(inst) = scene.anim_instance_mut(&want.name) {
                inst.time = want.time;
                inst.speed = want.speed;
                inst.playing = want.playing;
                restored += 1;
            }
        }
        restored
    }
}

/// 场景存档：场景源 + 运行时快照，一个文件装完一局。
///
/// 单独一个类型而不是往 [`SceneFile`] 里加字段，是为了保住两条既有契约：
/// [`SceneFile`] 是"场景**声明**"，`Scene::to_scene_file` 不写运行时状态这条
/// 对称性被单测锁着；存档是"这一局**的样子**"，语义不同，不该混进同一个结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneArchive {
    /// 格式标识（[`ARCHIVE_FORMAT`]）。
    #[serde(default = "archive_format")]
    pub format: String,
    /// 存档格式版本。
    #[serde(default = "archive_version")]
    pub version: u32,
    /// 场景源数据。
    pub scene: SceneFile,
    /// 运行时快照；旧存档 / 只要数据不要状态时可以缺省。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeSnapshot>,
}

impl SceneArchive {
    /// 从内存场景打一份存档（含运行时快照）。
    pub fn from_scene(scene: &Scene) -> Self {
        Self {
            format: ARCHIVE_FORMAT.to_string(),
            version: ARCHIVE_VERSION,
            scene: scene.to_scene_file(),
            runtime: Some(RuntimeSnapshot::capture(scene)),
        }
    }

    /// 只要场景源、不要运行时状态的存档（等价于"另存为场景"）。
    pub fn data_only(scene: &Scene) -> Self {
        Self {
            format: ARCHIVE_FORMAT.to_string(),
            version: ARCHIVE_VERSION,
            scene: scene.to_scene_file(),
            runtime: None,
        }
    }

    /// 版本校验。
    pub fn validate(&self) -> Result<(), String> {
        if self.version > ARCHIVE_VERSION {
            return Err(format!(
                "存档版本 {} 高于当前引擎支持的 {}，请升级引擎",
                self.version, ARCHIVE_VERSION
            ));
        }
        if let Some(rt) = &self.runtime {
            if !rt.is_compatible() {
                return Err(format!(
                    "存档运行时快照版本 {} 高于当前引擎支持的 {}",
                    rt.version, SNAPSHOT_VERSION
                ));
            }
        }
        Ok(())
    }

    /// 按存档重建场景，并（若有快照）恢复运行时状态。
    ///
    /// 返回值：`(场景, 恢复的动画实例数)`。场景源本身非法时返回 `Err`，
    /// 此时调用方**不应**动当前场景——先建筑再替换，避免半途失败留下半个世界。
    pub fn build_scene(&self) -> Result<(Scene, usize), String> {
        self.validate()?;
        let mut scene = Scene::from_file(self.scene.clone())?;
        let restored = match &self.runtime {
            Some(rt) => rt.apply(&mut scene),
            None => 0,
        };
        Ok((scene, restored))
    }

    /// 序列化为缩进 JSON（人工可读，便于排错与 diff）。
    pub fn to_json_pretty(&self) -> Result<String, String> {
        let mut text =
            serde_json::to_string_pretty(self).map_err(|e| format!("存档序列化失败: {e}"))?;
        text.push('\n');
        Ok(text)
    }

    /// 解析存档 JSON。
    ///
    /// 兼容两种输入：
    /// - 本模块写出的存档（顶层有 `scene` 段）；
    /// - **裸场景文件**（顶层直接就是 `objects` / `camera` …）。后者按"没有运行时
    ///   快照"处理——这样 `scene.json` 也能直接当存档喂进来，编辑器里少一次转换。
    pub fn from_json_str(src: &str) -> Result<Self, String> {
        let src = src.strip_prefix('\u{feff}').unwrap_or(src);
        let value: Value =
            serde_json::from_str(src).map_err(|e| format!("存档 JSON 解析失败: {e}"))?;
        if !value.is_object() {
            return Err("存档 JSON 顶层必须是对象".to_string());
        }
        if value.get("scene").is_some() {
            let archive: Self =
                serde_json::from_value(value).map_err(|e| format!("存档结构不匹配: {e}"))?;
            archive.validate()?;
            Ok(archive)
        } else {
            let scene: SceneFile =
                serde_json::from_value(value).map_err(|e| format!("场景结构不匹配: {e}"))?;
            Ok(Self {
                format: ARCHIVE_FORMAT.to_string(),
                version: ARCHIVE_VERSION,
                scene,
                runtime: None,
            })
        }
    }

    /// 写盘（自动建目录，结尾补换行，与 `Scene::save_to_path` 同一口径）。
    pub fn save_to_path(&self, path: &Path) -> Result<(), String> {
        let text = self.to_json_pretty()?;
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("创建目录 {} 失败: {e}", dir.display()))?;
            }
        }
        std::fs::write(path, text).map_err(|e| format!("写入存档 {} 失败: {e}", path.display()))
    }

    /// 从磁盘读存档。
    pub fn load_path(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("读取存档 {} 失败: {e}", path.display()))?;
        Self::from_json_str(&text)
    }
}

/// 栈里的一层：离开某个场景时的样子。
#[derive(Debug, Clone, PartialEq)]
pub struct StackEntry {
    /// 展示名（同名自动加 `#2` 后缀，见 [`SceneStack::unique_name`]）。
    pub name: String,
    /// 来源文件路径（脚本 / 编辑器现场构建的场景为空串）。
    pub path: String,
    /// 场景源文本（离开时的样子，`Scene::to_json_pretty` 口径）。
    pub source: String,
    /// 离开时的运行时状态。
    pub runtime: RuntimeSnapshot,
}

impl StackEntry {
    /// 以场景内存现状构造一层。
    pub fn capture(name: impl Into<String>, path: impl Into<String>, scene: &Scene) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            source: scene
                .to_json_pretty()
                .unwrap_or_else(|_| String::from("{}")),
            runtime: RuntimeSnapshot::capture(scene),
        }
    }
}

/// 场景栈：线性历史 + 游标。
///
/// 语义按"浏览器历史"设计而不是严格 LIFO：`back` 只是把游标往前挪，被切走的场景
/// 仍留在栈里，因此可以再 `forward` 回来。这条语义对游戏流程很关键——但愿
/// "回主菜单再进关卡"每次都要重新压栈、把历史越堆越长。
#[derive(Debug, Clone, Default)]
pub struct SceneStack {
    entries: Vec<StackEntry>,
    /// 当前所在层；`None` 表示栈为空。
    current: Option<usize>,
}

impl SceneStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前深度（层数）。
    pub fn depth(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 当前层的下标。
    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    /// 当前层。
    pub fn current(&self) -> Option<&StackEntry> {
        self.current.and_then(|i| self.entries.get(i))
    }

    /// 全部层（按压栈顺序，索引即层号）。
    pub fn entries(&self) -> &[StackEntry] {
        &self.entries
    }

    /// 层名列表（编辑器面板 / 调试输出用）。
    pub fn names(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.name.clone()).collect()
    }

    /// 用内存里的场景现状刷新**当前层**（离开这一层之前调用）。
    ///
    /// 层里存的是"离开时的样子"，而内存里的场景一直在跑；不刷新就 `back` 回去，
    /// 看到的会是进入这一层时的初始状态、而不是离开时的那一帧。返回 `false`
    /// 表示栈为空、没有可刷新的层。
    pub fn capture_current(&mut self, scene: &Scene) -> bool {
        let Some(idx) = self.current else {
            return false;
        };
        let Some(entry) = self.entries.get_mut(idx) else {
            return false;
        };
        entry.source = scene
            .to_json_pretty()
            .unwrap_or_else(|_| String::from("{}"));
        entry.runtime = RuntimeSnapshot::capture(scene);
        true
    }

    /// 清空。
    pub fn clear(&mut self) {
        self.entries.clear();
        self.current = None;
    }

    /// 压入一层并把它设为当前层；返回其层号。
    ///
    /// 与浏览器前进栈同理：从中间层压栈会**截断**它后面的层（那些是"还没走的路"，
    /// 一旦从当前层另开一条分支就没意义了）。超过 [`MAX_SCENE_STACK`] 时丢最老一层。
    pub fn push(&mut self, entry: StackEntry) -> usize {
        let keep = self.current.map(|i| i + 1).unwrap_or(0);
        self.entries.truncate(keep);
        self.entries.push(entry);
        while self.entries.len() > MAX_SCENE_STACK {
            self.entries.remove(0);
        }
        let idx = self.entries.len() - 1;
        self.current = Some(idx);
        idx
    }

    /// 回退一层；返回新的当前层（无更早的层时为 `None`）。
    pub fn back(&mut self) -> Option<&StackEntry> {
        let cur = self.current?;
        if cur == 0 {
            return None;
        }
        self.current = Some(cur - 1);
        self.current()
    }

    /// 前进一层；返回新的当前层（无更新的层时为 `None`）。
    pub fn forward(&mut self) -> Option<&StackEntry> {
        let cur = self.current?;
        let next = cur + 1;
        if next >= self.entries.len() {
            return None;
        }
        self.current = Some(next);
        self.current()
    }

    /// 跳到指定层。
    pub fn jump(&mut self, index: usize) -> Option<&StackEntry> {
        if index >= self.entries.len() {
            return None;
        }
        self.current = Some(index);
        self.current()
    }

    /// 按名字找层：**从当前层往回**找最近的同名层。
    ///
    /// 往回找而不是从头找，是因为"回到主菜单"应该回上一个主菜单，而不是历史里
    /// 最早那个。当前层本身也会被匹配到（幂等：切到自己就是原地不动）。
    pub fn find(&self, name: &str) -> Option<usize> {
        let cur = self.current?;
        self.entries[..=cur].iter().rposition(|e| e.name == name)
    }

    /// 按名字切层。
    pub fn switch_by_name(&mut self, name: &str) -> Option<&StackEntry> {
        let idx = self.find(name)?;
        self.jump(idx)
    }

    /// 弹出当前层；游标落到它前一层（没有前一层则清空游标）。
    pub fn pop_current(&mut self) -> Option<StackEntry> {
        let cur = self.current?;
        let entry = self.entries.remove(cur);
        if self.entries.is_empty() {
            self.current = None;
        } else {
            let idx = cur.saturating_sub(1).min(self.entries.len() - 1);
            self.current = Some(idx);
        }
        Some(entry)
    }

    /// 生成不重名的层名：`main` → `main#2` → `main#3` …
    pub fn unique_name(&self, base: &str) -> String {
        let base = if base.trim().is_empty() {
            "scene"
        } else {
            base
        };
        if !self.entries.iter().any(|e| e.name == base) {
            return base.to_string();
        }
        let mut n = 2;
        loop {
            let candidate = format!("{base}#{n}");
            if !self.entries.iter().any(|e| e.name == candidate) {
                return candidate;
            }
            n += 1;
        }
    }
}

/// 场景命令：宿主与脚本 / RPC 之间的「意图信封」（扩展十八 B）。
///
/// 切场景不是"读一份 JSON 再替换"那么轻：宿主必须按「压栈 → 换资产库 → 重建场景
/// （补挂资产节点、跑 rebuild hook）→ 重跑 `on_start`」的顺序做，而这套顺序是
/// `r3e-app` 的知识。脚本层与 RPC 层都不该知道宿主手里还有几本账，所以它们只能
/// **登记**一条命令，由宿主在帧首统一执行——这样「脚本在自己的 `on_start` 里申请
/// 切场景」也不会在重建到一半时把世界撕成两半。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneCommand {
    /// 切换到某个场景文件（相对路径按进程工作目录解析）。
    Switch { path: String },
    /// 回到栈里上一层。
    Back,
    /// 存档到文件；`None` 表示交给宿主挑默认位置。
    SaveArchive { path: Option<String> },
    /// 读回存档（既接受存档文件，也接受裸场景文件）。
    LoadArchive { path: String },
}

impl SceneCommand {
    pub fn switch(path: impl Into<String>) -> Self {
        Self::Switch { path: path.into() }
    }

    pub fn back() -> Self {
        Self::Back
    }

    pub fn save_archive(path: Option<String>) -> Self {
        Self::SaveArchive { path }
    }

    pub fn load_archive(path: impl Into<String>) -> Self {
        Self::LoadArchive { path: path.into() }
    }

    /// 命令标识（日志与 RPC 回执里的 `command` 字段）。
    pub fn label(&self) -> &'static str {
        match self {
            Self::Switch { .. } => "scene.switch",
            Self::Back => "scene.back",
            Self::SaveArchive { .. } => "scene.archive.save",
            Self::LoadArchive { .. } => "scene.archive.load",
        }
    }

    /// 是否会换掉当前场景。`SaveArchive` 只写盘，不动世界，因此不参与"每帧最多
    /// 换一次世界"的互斥——存档与切换可以在同一帧里排着队走。
    pub fn is_world_change(&self) -> bool {
        !matches!(self, Self::SaveArchive { .. })
    }

    /// 命令携带的路径（`Back` 没有路径，`SaveArchive` 可以没有）。
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Switch { path } | Self::LoadArchive { path } => Some(path.as_str()),
            Self::SaveArchive { path } => path.as_deref(),
            Self::Back => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::ObjectDesc;

    fn scene_with(names: &[&str]) -> Scene {
        let mut scene = Scene::new();
        scene.name = "test".to_string();
        for n in names {
            scene
                .spawn(ObjectDesc {
                    name: (*n).to_string(),
                    ..Default::default()
                })
                .expect("spawn 应成功");
        }
        scene
    }

    fn entry(name: &str, node: &str) -> StackEntry {
        StackEntry::capture(name, format!("{name}.json"), &scene_with(&[node]))
    }

    // ---------- 运行时快照 ----------

    #[test]
    fn snapshot_defaults_match_fresh_scene() {
        let scene = Scene::new();
        let snap = RuntimeSnapshot::capture(&scene);
        assert_eq!(snap.version, SNAPSHOT_VERSION);
        assert!(snap.text_enabled && snap.ui_enabled && snap.lights_enabled);
        assert!(!snap.ui_above_text);
        assert!(snap.anims.is_empty(), "无动画实例时不该凭空造");
        assert_eq!(snap, RuntimeSnapshot::default(), "抓取口径应与缺省一致");
    }

    #[test]
    fn snapshot_round_trips_through_json() {
        let mut scene = scene_with(&["a"]);
        scene.time = 12.5;
        scene.frame = 750;
        scene.text_enabled = false;
        scene.ui_above_text = true;
        let snap = RuntimeSnapshot::capture(&scene);
        let text = serde_json::to_string(&snap).expect("序列化");
        let back: RuntimeSnapshot = serde_json::from_str(&text).expect("反序列化");
        assert_eq!(snap, back);
    }

    #[test]
    fn snapshot_apply_overrides_flags_and_time() {
        let mut scene = scene_with(&["a"]);
        let snap = RuntimeSnapshot {
            version: SNAPSHOT_VERSION,
            time: 3.5,
            frame: 210,
            text_enabled: false,
            ui_enabled: false,
            lights_enabled: false,
            ui_above_text: true,
            anims: Vec::new(),
        };
        let restored = snap.apply(&mut scene);
        assert_eq!(restored, 0, "没有动画实例时恢复数为 0");
        assert_eq!(scene.time, 3.5);
        assert_eq!(scene.frame, 210);
        assert!(!scene.text_enabled && !scene.ui_enabled && !scene.lights_enabled);
        assert!(scene.ui_above_text);
    }

    #[test]
    fn snapshot_apply_skips_unknown_anim_instances() {
        let mut scene = scene_with(&["a"]);
        let snap = RuntimeSnapshot {
            anims: vec![AnimRuntime {
                name: "ghost/walk".to_string(),
                time: 1.0,
                speed: 1.0,
                playing: true,
            }],
            ..RuntimeSnapshot::default()
        };
        assert_eq!(snap.apply(&mut scene), 0, "名字匹配不上应静默跳过");
    }

    #[test]
    fn snapshot_rejects_future_version() {
        let snap = RuntimeSnapshot {
            version: SNAPSHOT_VERSION + 1,
            ..RuntimeSnapshot::default()
        };
        assert!(!snap.is_compatible());
    }

    // ---------- 存档 ----------

    #[test]
    fn archive_captures_source_and_runtime() {
        let mut scene = scene_with(&["hero"]);
        scene.time = 9.0;
        let archive = SceneArchive::from_scene(&scene);
        assert_eq!(archive.format, ARCHIVE_FORMAT);
        assert_eq!(archive.version, ARCHIVE_VERSION);
        assert_eq!(archive.scene.objects.len(), 1);
        assert_eq!(archive.runtime.expect("默认带快照").time, 9.0);
    }

    #[test]
    fn archive_data_only_omits_runtime() {
        let scene = scene_with(&["hero"]);
        let archive = SceneArchive::data_only(&scene);
        assert!(archive.runtime.is_none());
        let text = archive.to_json_pretty().expect("序列化");
        assert!(
            !text.contains("\"runtime\""),
            "只要数据时不该写出 runtime 段:\n{text}"
        );
    }

    #[test]
    fn archive_round_trip_restores_scene_and_clock() {
        let mut scene = scene_with(&["hero", "floor"]);
        scene.time = 4.25;
        scene.frame = 255;
        scene.text_enabled = false;
        let archive = SceneArchive::from_scene(&scene);
        let text = archive.to_json_pretty().expect("序列化");

        let back = SceneArchive::from_json_str(&text).expect("反序列化");
        let (rebuilt, restored) = back.build_scene().expect("重建场景");
        assert_eq!(rebuilt.len(), 2, "物件数应一致");
        assert_eq!(rebuilt.time, 4.25, "运行时时间应恢复");
        assert_eq!(rebuilt.frame, 255);
        assert!(!rebuilt.text_enabled);
        assert_eq!(restored, 0, "无动画实例");
    }

    #[test]
    fn archive_accepts_raw_scene_file() {
        let src = scene_with(&["solo"]).to_json_string().expect("序列化场景");
        let archive = SceneArchive::from_json_str(&src).expect("裸场景文件应被接受");
        assert!(archive.runtime.is_none(), "裸场景没有运行时快照");
        assert_eq!(archive.scene.objects.len(), 1);
    }

    #[test]
    fn archive_rejects_future_version_and_bad_json() {
        let src = format!(
            "{{\"format\":\"{ARCHIVE_FORMAT}\",\"version\":{},\"scene\":{{}}}}",
            ARCHIVE_VERSION + 1
        );
        let err = SceneArchive::from_json_str(&src).expect_err("高版本应被拒");
        assert!(err.contains("存档版本"), "{err}");

        assert!(SceneArchive::from_json_str("not json").is_err());
        assert!(
            SceneArchive::from_json_str("[1,2]").is_err(),
            "顶层必须对象"
        );
    }

    #[test]
    fn archive_save_and_load_round_trip_on_disk() {
        let scene = scene_with(&["disk"]);
        let archive = SceneArchive::from_scene(&scene);
        let dir = std::env::temp_dir().join(format!("r3e-archive-{}", std::process::id()));
        let path = dir.join("nested/slot1.json");
        archive.save_to_path(&path).expect("写盘");
        let back = SceneArchive::load_path(&path).expect("读盘");
        assert_eq!(
            back.to_json_pretty().expect("序列化"),
            archive.to_json_pretty().expect("序列化"),
            "落盘往返必须等价"
        );
        assert_eq!(back.scene.objects.len(), archive.scene.objects.len());

        std::fs::remove_file(&path).expect("清理临时文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------- 栈 ----------

    #[test]
    fn stack_starts_empty() {
        let stack = SceneStack::new();
        assert_eq!(stack.depth(), 0);
        assert!(stack.is_empty());
        assert_eq!(stack.current_index(), None);
        assert!(stack.current().is_none());
    }

    #[test]
    fn stack_push_makes_new_entry_current() {
        let mut stack = SceneStack::new();
        assert_eq!(stack.push(entry("main", "menu")), 0);
        assert_eq!(stack.push(entry("level1", "hero")), 1);
        assert_eq!(stack.depth(), 2);
        assert_eq!(stack.current_index(), Some(1));
        assert_eq!(stack.current().expect("有当前层").name, "level1");
    }

    #[test]
    fn stack_back_and_forward_move_cursor_only() {
        let mut stack = SceneStack::new();
        stack.push(entry("main", "menu"));
        stack.push(entry("level1", "hero"));

        assert_eq!(stack.back().expect("可回退").name, "main");
        assert_eq!(stack.depth(), 2, "回退不删层");
        assert_eq!(stack.back(), None, "已在最老层");

        assert_eq!(stack.forward().expect("可前进").name, "level1");
        assert_eq!(stack.forward(), None, "已是最新层");
    }

    #[test]
    fn stack_push_after_back_truncates_forward_branch() {
        let mut stack = SceneStack::new();
        stack.push(entry("main", "menu"));
        stack.push(entry("level1", "hero"));
        stack.back();
        stack.push(entry("settings", "ui"));
        assert_eq!(stack.names(), vec!["main", "settings"], "前进分支被截断");
        assert_eq!(stack.current_index(), Some(1));
    }

    #[test]
    fn stack_drops_oldest_entry_beyond_limit() {
        let mut stack = SceneStack::new();
        for i in 0..(MAX_SCENE_STACK + 3) {
            stack.push(entry(&format!("s{i}"), "n"));
        }
        assert_eq!(stack.depth(), MAX_SCENE_STACK, "超限丢最老");
        assert_eq!(stack.current_index(), Some(MAX_SCENE_STACK - 1));
        assert_eq!(stack.entries()[0].name, "s3", "最早三层被丢弃");
    }

    #[test]
    fn stack_find_searches_backwards_from_current() {
        let mut stack = SceneStack::new();
        stack.push(entry("main", "menu"));
        stack.push(entry("level1", "hero"));
        stack.push(entry("level2", "hero"));
        stack.push(entry("main", "menu"));
        assert_eq!(stack.find("main"), Some(3), "当前层的同名项优先");

        stack.back();
        stack.back();
        assert_eq!(stack.find("main"), Some(0), "回退后找到更早的同名层");
        assert_eq!(stack.find("level2"), None, "当前之后的不算历史");
        assert_eq!(stack.switch_by_name("main").expect("可切").name, "main");
        assert_eq!(stack.current_index(), Some(0));
        assert_eq!(stack.switch_by_name("nope"), None);
    }

    #[test]
    fn stack_jump_rejects_out_of_range() {
        let mut stack = SceneStack::new();
        stack.push(entry("main", "menu"));
        assert!(stack.jump(1).is_none());
        assert_eq!(stack.jump(0).expect("合法层号").name, "main");
    }

    #[test]
    fn stack_pop_current_moves_cursor_to_previous() {
        let mut stack = SceneStack::new();
        stack.push(entry("main", "menu"));
        stack.push(entry("level1", "hero"));
        let popped = stack.pop_current().expect("弹出一层");
        assert_eq!(popped.name, "level1");
        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.current_index(), Some(0));

        stack.pop_current();
        assert_eq!(stack.depth(), 0);
        assert_eq!(stack.current_index(), None, "空栈没有当前层");
        assert!(stack.pop_current().is_none());
    }

    #[test]
    fn stack_unique_name_appends_suffix() {
        let mut stack = SceneStack::new();
        assert_eq!(stack.unique_name("main"), "main");
        stack.push(entry("main", "menu"));
        assert_eq!(stack.unique_name("main"), "main#2");
        stack.push(entry("main#2", "menu"));
        assert_eq!(stack.unique_name("main"), "main#3");
        assert_eq!(stack.unique_name("   "), "scene", "空名有兜底");
    }

    #[test]
    fn stack_entry_capture_keeps_source_and_runtime() {
        let mut scene = scene_with(&["solo"]);
        scene.time = 1.5;
        let captured = StackEntry::capture("lv", "lv.json", &scene);
        assert_eq!(captured.name, "lv");
        assert_eq!(captured.path, "lv.json");
        assert_eq!(captured.runtime.time, 1.5);

        let back = Scene::from_json_str(&captured.source).expect("源文本必须可回读");
        assert_eq!(back.len(), 1, "源文本应带上物件");
        assert_eq!(back.time, 0.0, "源文本按对称口径不写运行时时间");
    }

    #[test]
    fn stack_clear_resets_everything() {
        let mut stack = SceneStack::new();
        stack.push(entry("main", "menu"));
        stack.clear();
        assert!(stack.is_empty());
        assert_eq!(stack.current_index(), None);
    }

    // ---------- 场景命令 ----------

    #[test]
    fn command_labels_are_stable() {
        assert_eq!(SceneCommand::switch("a.json").label(), "scene.switch");
        assert_eq!(SceneCommand::back().label(), "scene.back");
        assert_eq!(
            SceneCommand::save_archive(None).label(),
            "scene.archive.save"
        );
        assert_eq!(
            SceneCommand::load_archive("slot.json").label(),
            "scene.archive.load"
        );
    }

    #[test]
    fn command_path_follows_variant() {
        assert_eq!(SceneCommand::switch("a.json").path(), Some("a.json"));
        assert_eq!(SceneCommand::back().path(), None);
        assert_eq!(SceneCommand::save_archive(None).path(), None);
        assert_eq!(
            SceneCommand::save_archive(Some("slot.json".into())).path(),
            Some("slot.json")
        );
        assert_eq!(
            SceneCommand::load_archive("slot.json").path(),
            Some("slot.json")
        );
    }

    #[test]
    fn only_save_archive_is_not_a_world_change() {
        assert!(SceneCommand::switch("a.json").is_world_change());
        assert!(SceneCommand::back().is_world_change());
        assert!(SceneCommand::load_archive("slot.json").is_world_change());
        assert!(!SceneCommand::save_archive(None).is_world_change());
        assert!(!SceneCommand::save_archive(Some("slot.json".into())).is_world_change());
    }

    #[test]
    fn command_carries_text_verbatim() {
        // 命令只是信封：不做 trim、不做规范化，路径语义留给宿主（相对路径按 cwd 解析）。
        let cmd = SceneCommand::switch("  scenes/a.json  ");
        assert_eq!(cmd.path(), Some("  scenes/a.json  "));
        assert_eq!(
            cmd,
            SceneCommand::Switch {
                path: "  scenes/a.json  ".into()
            }
        );
    }

    #[test]
    fn capture_current_refreshes_only_the_top() {
        let mut stack = SceneStack::new();
        assert!(!stack.capture_current(&Scene::new()), "空栈没有可刷新的层");

        stack.push(StackEntry::capture("a", "a.json", &Scene::new()));
        stack.push(StackEntry::capture("b", "b.json", &Scene::new()));

        let mut live = Scene::new();
        live.name = "b-live".into();
        live.frame = 42;
        assert!(stack.capture_current(&live));

        assert_eq!(stack.depth(), 2);
        assert_eq!(stack.current_index(), Some(1));
        let top = stack.current().expect("当前层应存在");
        assert!(top.source.contains("b-live"), "层里应存下改名后的现状");
        assert_eq!(top.runtime.frame, 42);
        // 上一层不受影响
        assert!(!stack.entries()[0].source.contains("b-live"));
    }
}
