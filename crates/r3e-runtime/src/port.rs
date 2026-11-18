//! 编辑器宿主的**端口**：主循环把每帧可变状态打包成 [`EngineAccess`] 交出去，
//! 由实现了 [`EditorService`] 的适配器把协议请求翻译成宿主操作。
//!
//! 为什么是「主循环推、服务端拉」而不是「服务端持有 `&mut App`」：
//! 依赖方向必须是 `editor-host → runtime`（宿主知道自己活在哪个运行时里，反之不成立），
//! 而 `runtime` 又不能依赖 `r3e-editor`（协议层的存在不该被主循环感知）。
//! 于是主循环只认这个 trait：它不知道对面是 TCP + JSON-RPC，还是测试里的假服务端。
//!
//! 生命周期也顺带变干净了：`EngineAccess` 是**一次请求一份**的借用包，
//! 用完即弃，不存在「服务端长期持有场景引用」导致主循环借不到 `&mut` 的僵局。

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use r3e_core::MeshProvider;
use r3e_doc::Document;
use r3e_physics::Physics;
use r3e_render::Renderer;
use r3e_script::ScriptHost;

use crate::time::FrameTimeStats;

/// 一次编辑器请求可见的宿主状态。
///
/// 字段与原 `LocalEngineApi` 一一对应：这里刻意**只装借用**，不装所有权，
/// 保证请求处理结束时宿主立刻恢复独占，主循环下一帧照常推进。
pub struct EngineAccess<'a> {
    pub scene: &'a Rc<RefCell<Document>>,
    pub physics: &'a Rc<RefCell<Physics>>,
    pub scripts: &'a mut ScriptHost,
    /// 网格来源（M1-S4-G）：与主循环装配刚体时**同一个**「资产优先、图元兜底」口径。
    ///
    /// 为什么给 `Arc` 而不是借用：装配发生在 `Document::write` 闭包里，借用会和
    /// 外层 `RefCell` 打架；`Arc` 克隆是 O(1)，一次请求一份也不算成本。
    ///
    /// `None` = 该宿主拿不到网格来源（headless 测试）：需要形状的物理复核必须
    /// 明确回报「装配不了」，而不是拿包围盒猜出来的形状顶上去。
    pub meshes: Option<Arc<dyn MeshProvider>>,
    /// 窗口 / GPU 未就绪时为 `None`，此时依赖渲染器的请求应明确回报「不可用」。
    pub renderer: Option<&'a mut Renderer>,
    pub frame_ms: FrameTimeStats,
    /// 截图默认落盘目录（`capture.frame` 不传 path 时用）。
    pub capture_dir: PathBuf,
    pub scene_label: String,
    pub script_label: String,
    pub frames: u64,
    pub fps: f32,
    pub uptime_ms: u64,
    pub paused: bool,
}

/// 编辑器请求的服务端抽象。
///
/// 实现者负责「有没有待办请求」与「怎么执行一条」，主循环只负责按
/// `rpc_max_per_frame` 逐条喂 [`EngineAccess`]。
pub trait EditorService {
    /// 处理一条待办请求。
    ///
    /// 返回 `true` = 本条已处理，可以继续取下一条；`false` = 当前没有待办，
    /// 主循环本帧到此为止。把「判空」和「执行」合成一次调用，是为了让每次执行
    /// 都拿到一份全新的 `EngineAccess`——借用包不能跨请求复用。
    fn poll_one(&mut self, access: EngineAccess<'_>) -> bool;

    /// 服务端监听地址（用于启动日志）；未监听时为 `None`。
    fn local_addr(&self) -> Option<String>;
}
