//! `RpcServer` → [`EditorService`] 的适配器。
//!
//! 方向只有一个：主循环每帧调用 [`EditorService::poll_one`]，我们把当帧的
//! 可变状态打包成 `EngineAccess`，再交给协议层排队好的 `Job`。协议层拿不到
//! 也不持有场景引用，所以不会出现「编辑器攥着 `&mut` 导致主循环借不到」的僵局。

use std::cell::RefCell;

use r3e_editor::RpcServer;
use r3e_nav::Nav;
use r3e_runtime::{EditorService, EngineAccess};

use crate::api::LocalEngineApi;

/// 把 JSON-RPC 服务端包装成主循环可用的编辑器挂件。
pub struct RpcHost {
    server: RpcServer,
    /// 导航网格槽位（M1-S4-D）：`nav.bake` 写入、`nav.path` / `nav.reachability` 读取。
    ///
    /// 由宿主持有、进程内借用，**不进 `Document`**：导航是烘焙产物而非场景数据，
    /// 混进场景会让烘焙污染 `state_hash` 与撤销栈。
    nav: RefCell<Option<Nav>>,
}

impl RpcHost {
    /// 绑定监听端口（`0` = 由系统分配，实际端口用 [`Self::local_addr`] 查）。
    pub fn bind(port: u16, token: Option<String>) -> std::io::Result<Self> {
        let server = RpcServer::bind(port, token)?;
        log::info!("编辑器桥已就绪: {}", server.local_addr());
        Ok(Self {
            server,
            nav: RefCell::new(None),
        })
    }

    /// 底层服务端（测试与诊断要查连接数 / 令牌）。
    pub fn server(&self) -> &RpcServer {
        &self.server
    }
}

impl EditorService for RpcHost {
    fn poll_one(&mut self, access: EngineAccess<'_>) -> bool {
        // 一帧一条：队列里剩下的留到下一帧，主循环才是节奏的掌控者。
        let Some(job) = self.server.try_recv() else {
            return false;
        };
        let mut api = LocalEngineApi {
            scene: access.scene,
            physics: access.physics,
            audio: access.audio,
            // 槽位生命周期与 `RpcHost` 同步：同一进程里反复烘焙复用同一格，
            // 「上一次烘的网格」在没人要求时不会被悄悄丢弃。
            nav: Some(&self.nav),
            scripts: access.scripts,
            // 网格来源由运行时按「资产优先、图元兜底」包装好递进来（M1-S4-G）：
            // 宿主不自己造 provider，否则影子装配与主循环装配迟早看两个不同的网格。
            meshes: access.meshes,
            renderer: access.renderer,
            frame_ms: access.frame_ms,
            capture_dir: access.capture_dir,
            scene_label: access.scene_label,
            script_label: access.script_label,
            frames: access.frames,
            fps: access.fps,
            uptime_ms: access.uptime_ms,
            paused: access.paused,
        };
        // 响应由协议层自己回写（它拿着连接）；这里只负责在正确的线程、
        // 正确的帧上把请求执行掉。
        job.run(&mut api);
        true
    }

    fn local_addr(&self) -> Option<String> {
        Some(self.server.local_addr().to_string())
    }
}
