//! `physics.*` 三条查询的参数承载（M1-S4-D）。
//!
//! 协议层**零引擎依赖**（`Cargo.toml` 里只有 serde / serde_json），所以这里不出现
//! `Vec3` / `Ray` / `CastShape`：一律用 `[f32; 3]` 与字符串标签收敛形状，由宿主翻译成
//! 引擎类型。与 `query.rs` 同口径——**判断与缺省不前置到协议层**：
//!
//! * `maxDistance` 留 `None` 到宿主：宿主取自己的缺省射程常量，
//!   改常量不会漏改协议层，也不会让「协议默认 100 米」变成第二个真源。
//! * `shape` 用标签 + 尺寸字段（`{"kind": "sphere", "radius": 0.3}`），不做成
//!   `CastShape` 的镜像枚举：少一层字段名同步，新增形状在宿主侧一处收口。
//! * 方向的归一化、`maxDistance <= 0` 的处理都留给宿主——协议层只搬值，
//!   免得同一个非法入参在两层各有一套「容错」，最终谁都不认账。

/// `physics.raycast` 的入参。
#[derive(Debug, Clone, PartialEq)]
pub struct RayQuery {
    /// 起点（世界坐标）。
    pub origin: [f32; 3],
    /// 方向（不要求归一化，宿主归一化）。
    pub dir: [f32; 3],
    /// 射程；`None` = 宿主缺省。
    pub max_distance: Option<f32>,
}

/// `physics.overlap` 的入参：球体覆盖查询。
#[derive(Debug, Clone, PartialEq)]
pub struct OverlapQuery {
    /// 球心（世界坐标）。
    pub center: [f32; 3],
    /// 半径（> 0；`<= 0` 由宿主判参数错误）。
    pub radius: f32,
    /// 命中条数上限；`None` = 宿主缺省，`0` = 不限。
    pub limit: Option<usize>,
}

/// `physics.cast` 的入参：把球 / 盒沿方向推过去，拿最近一次接触。
#[derive(Debug, Clone, PartialEq)]
pub struct CastQuery {
    /// 起点（世界坐标）。
    pub origin: [f32; 3],
    /// 方向（不要求归一化，宿主归一化）。
    pub dir: [f32; 3],
    /// 射程；`None` = 宿主缺省。
    pub max_distance: Option<f32>,
    /// 扫掠形状。
    pub shape: CastShapeSpec,
}

/// 扫掠形状（协议侧形状收敛）。
///
/// `half` 是**半尺寸**，与 `r3e_physics::BodyDesc::half_extents` 同口径——用整尺寸
/// 会得到一个看着大一圈、实际早撞上的碰撞体，这种「差一倍」的错误在联调时极难发现。
#[derive(Debug, Clone, PartialEq)]
pub enum CastShapeSpec {
    /// 球：负半径由宿主取绝对值。
    Sphere { radius: f32 },
    /// 轴对齐盒。
    Box { half: [f32; 3] },
}
