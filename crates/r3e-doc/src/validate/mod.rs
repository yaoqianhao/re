//! 场景自洽校验（S3-P3 Validator v0）。
//!
//! **口径**：只查「场景自己说得通吗」，不碰外部资产库。渲染侧的网格 / 材质 / 骨骼解析
//! 要走资产导入（`MeshAabbCache` / `MeshData`），那部分留到 M1 的 Validator v1；
//! 本轮把能在场景结构内部判死的问题全部抓出来，让 `doc-gate` 之外多一道运行时兜底。
//!
//! 三条铁律：
//! 1. **不修复、不 panic、不短路**——一次扫描报全量问题，交给上层决定怎么处理；
//! 2. **只读**——只吃 `&Scene`，不碰写入口，不会因为校验把文档标脏；
//! 3. **顺序稳定**——结果按 `path` 排序，同 path 按 `code`，测试可逐条断言。

use std::fmt;

use r3e_core::Scene;

mod bounds;
mod character;
mod geom;
mod invariants;
mod material;
mod overlap;
mod prefab;
mod refs;
mod support;
mod terrain;

pub use invariants::{by_code as issue_code_by_code, IssueCode, ISSUE_CODES};

/// 问题严重度。
///
/// `Error` = 场景自相矛盾，跑下去必然出错（引用悬空、key 重复、数值非有限）；
/// `Warn` = 可疑但可用（例如蒙皮尚未求解过关节矩阵）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warn,
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Warn => write!(f, "warn"),
            Self::Error => write!(f, "error"),
        }
    }
}

/// 问题归类（M1-S4-E）。
///
/// 与 `code` 的分工：`code` 是**具体判据**（一条判据一个码），`category` 是**问题族**。
/// 三层验证器（静态 / 物理 / 导航）会对同一对象报同一族的问题，族 + 对象 + 码
/// 三者合起来才是去重键——见 [`dedup_superseded`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    /// 引用断裂（节点 → 父物件 / 材质 / 动画 / 脚本 …）
    Reference,
    /// 数值越界、非有限、容量超限
    Bounds,
    /// 材质声明与其贴图路径
    Material,
    /// 悬空（下方没有支撑面）
    Support,
    /// 穿模（体积交叠）
    Overlap,
    /// 可达性（出生点 / 目标 / 连通块）
    Reachability,
    /// 角色控制器（配置越界 / 埋进支撑面）
    Character,
    /// 地形（样本不可归一 / 地表空洞）
    Terrain,
    /// Prefab（覆盖字段写回模板 / 指向不存在的模板，M2-E04）
    Prefab,
}

impl Category {
    /// 机器可读名，进 JSON / capabilities 时用它。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reference => "reference",
            Self::Bounds => "bounds",
            Self::Material => "material",
            Self::Support => "support",
            Self::Overlap => "overlap",
            Self::Reachability => "reachability",
            Self::Character => "character",
            Self::Terrain => "terrain",
            Self::Prefab => "prefab",
        }
    }

    /// 由 `code` 前缀反推归类。
    ///
    /// 这样做的理由：`code` 是稳定契约（登记在 [`ISSUE_CODES`]、被 `xtask invariants` 锁死），
    /// 前缀与族一一对应；三十余处既有 `Issue::error(code, …)` 调用点因此零改动。
    pub fn from_code(code: &str) -> Self {
        let head = code.split('.').next().unwrap_or(code);
        match head {
            "ref" => Self::Reference,
            "material" => Self::Material,
            "support" => Self::Support,
            "overlap" => Self::Overlap,
            "nav" => Self::Reachability,
            "character" => Self::Character,
            "terrain" => Self::Terrain,
            "prefab" => Self::Prefab,
            _ => Self::Bounds,
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 单条问题。
///
/// `code` 是稳定的机器可读标识（测试与上层筛选都按它来），不要为了文案好看去改；
/// 文案走 `msg`。
///
/// **坐标化（M1-S4-E）**：`location` 是失败点位的世界坐标——验收口径要的是
/// 「报出具体点位坐标，不是布尔值」，所以三层验证器每一条空间类结论都必须带它。
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    pub code: &'static str,
    /// 问题族，见 [`Category`]。
    pub category: Category,
    /// 出问题的位置，形如 `nodes[3]`、`animations[0].bindings[2].node`、`ui[4].key`。
    pub path: String,
    /// 问题对象的名字（节点名 / 材质名 / `"a|b"` 这样的对象对）。
    ///
    /// 去重键的一半：跨层对「同一对象」下结论时，靠它把近似结论被动精确结论顶掉。
    pub target: Option<String>,
    pub msg: String,
    /// 失败点位（世界坐标），没有空间语义的问题为 `None`。
    pub location: Option<[f32; 3]>,
    /// 修复建议，一句话、可执行。
    pub hint: Option<String>,
    /// 本结论是否基于近似几何（AABB）。
    ///
    /// `true` = 静态层初筛，物理 / 导航层给出同 `(category,target,code)` 的精确结论时
    /// 必须被覆盖；`false` = 精确判定。
    pub approximate: bool,
}

impl Issue {
    /// 构造一条 `Error` 级问题。
    ///
    /// 公开给三层验证器的物理 / 导航层（`r3e-editor-host/src/verify`）用：它们在别的
    /// crate 里下结论，但结论的类型必须与静态层**是同一个** `Issue`，否则上层要写两套
    /// 解析、两套去重。`code` 必须已在 [`ISSUE_CODES`] 登记（`xtask invariants` 会查）。
    pub fn error(code: &'static str, path: impl Into<String>, msg: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, path, msg)
    }

    /// 构造一条 `Warn` 级问题（用法同 [`Issue::error`]）。
    pub fn warn(code: &'static str, path: impl Into<String>, msg: impl Into<String>) -> Self {
        Self::new(Severity::Warn, code, path, msg)
    }

    fn new(
        severity: Severity,
        code: &'static str,
        path: impl Into<String>,
        msg: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            code,
            category: Category::from_code(code),
            path: path.into(),
            target: None,
            msg: msg.into(),
            location: None,
            hint: None,
            approximate: false,
        }
    }

    /// 绑定问题对象名（去重键用）。
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// 绑定失败点位坐标。
    pub fn at(mut self, location: [f32; 3]) -> Self {
        self.location = Some(location);
        self
    }

    /// 绑定修复建议。
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// 标记为近似结论（静态层 AABB 初筛）。
    pub fn approx(mut self) -> Self {
        self.approximate = true;
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {} @ {}：{}",
            self.severity, self.code, self.path, self.msg
        )?;
        if let Some([x, y, z]) = self.location {
            write!(f, "（点位 {x:.3}, {y:.3}, {z:.3}）")?;
        }
        if self.approximate {
            write!(f, "（近似）")?;
        }
        Ok(())
    }
}

/// 校验整份场景。返回全部问题（可能为空），顺序稳定。
pub fn validate(scene: &Scene) -> Vec<Issue> {
    let mut out = Vec::new();
    refs::check(scene, &mut out);
    bounds::check(scene, &mut out);
    material::check(scene, &mut out);
    character::check(scene, &mut out);
    // 地形族（M2-C06）：同为**确定**结论——样本数与分辨率、格覆盖都由场景内真值
    // 算得出，物理层没有更准的第二份口径去复核。三个入口（加载期 / 验证期 / 带目录）
    // 必须一致，所以三处都调。
    terrain::check(scene, &mut out);
    // Prefab 族（M2-E04）：模板存在性与「覆盖是否抄回模板值」同样是**确定结论**，
    // 与地形族同理三入口一致，所以加载期 / 验证期 / 带目录三处都调。
    prefab::check(scene, &mut out);
    sort_issues(&mut out);
    out
}

/// 静态层几何初筛（M1-S4-E）：**悬空 + 穿模**，只有 AABB 近似。
///
/// 与 [`validate`] 分开入口，不是偷懒，是口径不同：
///
/// - [`validate`] 判的是「场景自相矛盾」（引用悬空、数值非有限），结论**确定**，
///   一条都不能漏，所以每次加载必跑；
/// - 本函数判的是「空间关系可疑」，结论**全是近似**（没有资产库，拿不到真实包围盒），
///   只该在验证链里跑，并且必须由物理层复核覆盖。
///
/// 混进 [`validate`] 会让每一次场景加载都多出一批需要下游自己筛掉的近似条目，
/// 也会让 `Warn` 语义退化成噪声。三层组装在 `r3e-editor-host/src/verify`。
pub fn validate_geometry(scene: &Scene) -> Vec<Issue> {
    let mut out = Vec::new();
    support::check(scene, &mut out);
    overlap::check(scene, &mut out);
    // 角色族（M2-B04）：`character.*` 是**确定**结论，不是近似候选——脚底与支撑面
    // 都由场景内真值算得出，物理层没有更准的第二份口径去复核它。放这里是因为
    // 三层验证报告（`scene.verify`）必须以静态层为入口把这两条码交出去；
    // 只挂 `validate` 的话，加载期看得到、验证期反而看不到，同一份场景给两个结论。
    // 它们是精确条目（`approximate = false`），不会参与「近似被覆盖」那条规则。
    character::check(scene, &mut out);
    // 地形族（M2-C06）：同为**确定**结论——样本数与分辨率、格覆盖都由场景内真值
    // 算得出，物理层没有更准的第二份口径去复核。三个入口（加载期 / 验证期 / 带目录）
    // 必须一致，所以三处都调。
    terrain::check(scene, &mut out);
    // Prefab 族（M2-E04）：模板存在性与「覆盖是否抄回模板值」同样是**确定结论**，
    // 与地形族同理三入口一致，所以加载期 / 验证期 / 带目录三处都调。
    prefab::check(scene, &mut out);
    sort_issues(&mut out);
    dedup_superseded(out)
}

/// 节点的近似世界包围盒（M1-S4-E 三层验证器的共用几何入口）。
///
/// 静态层自己用，物理层也用同一份——装配口径（声明 `half_extents` 优先、否则
/// `0.5 × |scale|`）只有一处实现，层间才不会对「这个物件多大」有分歧。
pub fn node_bounds(node: &r3e_core::Node) -> Option<r3e_core::Aabb> {
    geom::node_aabb(node)
}

/// 该物件是否会在 run 起来后自己动（`body = "dynamic"`）。
///
/// 动态体不判悬空、不判穿模：它们下一刻在哪由模拟决定，静态结论只会变成噪声。
pub fn is_dynamic_node(node: &r3e_core::Node) -> bool {
    geom::is_dynamic(node)
}

/// 两个节点是否有父子关系（祖先 / 后代）。
///
/// 父子体积重叠是层次结构的正常现象，不该报穿模；父子之间的「悬空」同理
/// （子物件挂在父物件上，支撑方是父物件本身）。
pub fn nodes_related(scene: &Scene, a: r3e_core::NodeId, b: r3e_core::NodeId) -> bool {
    match (scene.get(a), scene.get(b)) {
        (Some(a), Some(b)) => geom::related(scene, a, b),
        _ => false,
    }
}

/// 稳定排序：`path` → `code` → `msg`，同键按坐标兜底（三层合并后可能出现同 path 同码两条）。
fn sort_issues(out: &mut [Issue]) {
    out.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.code.cmp(b.code))
            .then_with(|| a.msg.cmp(&b.msg))
            .then_with(|| {
                let key = |i: &Issue| {
                    i.location
                        .map(|l| (l[0].to_bits(), l[1].to_bits(), l[2].to_bits()))
                };
                key(a).cmp(&key(b))
            })
    });
}

/// 精确结论覆盖近似结论，并把同键重复的条目压成一条（M1-S4-E 铁律①②）。
///
/// 覆盖按 **`(category, target)` 族级**判定，而不是按 `code`：
///
/// 静态层说「AABB 疑似悬空」用 `support.suspect`，物理层精确复核后说「确实悬空」用
/// `support.floating`——两条码不同、判据不同、结论方向也不同。若只按 `code` 去重，
/// 两条会一起留下，AI 会收到「疑似悬空 + 确定悬空」两条同族结论，或者更糟：
/// 收到「疑似悬空」而精确层其实已经否掉了它（精确层不报 = 没有这条，近似条就赖着不走）。
///
/// 所以口径是：**同一问题族、同一对象，只要有一层给出了精确结论，该族的近似条目全部丢弃**；
/// 精确条目之间再按 `(category, target, code)` 压重复。
///
/// - 没有 `target` 的条目（容量超限、汇总提示这类「不对应单个对象」的问题）不参与覆盖，
///   也不会被压掉；
/// - 内部先排序再压，结果可复现（调用方不必自己保证顺序）。
pub fn dedup_superseded(mut issues: Vec<Issue>) -> Vec<Issue> {
    use std::collections::BTreeSet;

    sort_issues(&mut issues);

    let mut exact: BTreeSet<(Category, String)> = BTreeSet::new();
    for issue in &issues {
        if !issue.approximate {
            if let Some(target) = &issue.target {
                exact.insert((issue.category, target.clone()));
            }
        }
    }

    let mut seen: BTreeSet<(Category, String, &'static str)> = BTreeSet::new();
    issues
        .into_iter()
        .filter(|issue| match &issue.target {
            Some(target) => {
                if issue.approximate && exact.contains(&(issue.category, target.clone())) {
                    return false;
                }
                seen.insert((issue.category, target.clone(), issue.code))
            }
            None => true,
        })
        .collect()
}

/// 地形高度采样（世界 x/z → 最高一层地形面高度）。
///
/// 三层验证器共用同一份地形真值：静态层判支撑、物理层判悬空、导航层对拍烘焙结果，
/// 谁都不许自己再造一个采样公式，否则「视觉上踩在地上、验证器说悬空」这种活见鬼结论
/// 迟早出现。没有任何地形块覆盖该点时返回 `None`（不是 `0.0`——平地与「没有地形」
/// 对判据的影响完全不同）。
pub fn terrain_height_at(scene: &Scene, x: f32, z: f32) -> Option<f32> {
    geom::terrain_height(scene, x, z)
}

/// 带场景目录的完整校验：在 [`validate`] 之上额外判定 `materials[].*` 贴图文件是否存在。
///
/// `dir` 是场景文件所在目录（相对贴图路径的解析基准）。只读文件元数据，不解码、不改场景。
pub fn validate_in_dir(scene: &Scene, dir: &std::path::Path) -> Vec<Issue> {
    let mut out = Vec::new();
    refs::check(scene, &mut out);
    bounds::check(scene, &mut out);
    material::check_in_dir(scene, dir, &mut out);
    character::check(scene, &mut out);
    // 地形族（M2-C06）：同为**确定**结论——样本数与分辨率、格覆盖都由场景内真值
    // 算得出，物理层没有更准的第二份口径去复核。三个入口（加载期 / 验证期 / 带目录）
    // 必须一致，所以三处都调。
    terrain::check(scene, &mut out);
    // Prefab 族（M2-E04）：模板存在性与「覆盖是否抄回模板值」同样是**确定结论**，
    // 与地形族同理三入口一致，所以加载期 / 验证期 / 带目录三处都调。
    prefab::check(scene, &mut out);
    sort_issues(&mut out);
    out
}

/// 只要 `Error`，过滤掉 `Warn`。上层「能不能继续跑」的判据用这个。
pub fn errors(scene: &Scene) -> Vec<Issue> {
    validate(scene)
        .into_iter()
        .filter(Issue::is_error)
        .collect()
}
