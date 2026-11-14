//! `RpcServer` → [`EditorService`] 的适配器。
//!
//! 方向只有一个：主循环每帧调用 [`EditorService::poll_one`]，我们把当帧的
//! 可变状态打包成 `EngineAccess`，再交给协议层排队好的 `Job`。协议层拿不到
//! 也不持有场景引用，所以不会出现「编辑器攥着 `&mut` 导致主循环借不到」的僵局。

use r3e_editor::RpcServer;
use r3e_runtime::{EditorService, EngineAccess};

use crate::api::LocalEngineApi;

/// 把 JSON-RPC 服务端包装成主循环可用的编辑器挂件。
pub struct RpcHost {
    server: RpcServer,
}

impl RpcHost {
    /// 绑定监听端口（`0` = 由系统分配，实际端口用 [`Self::local_addr`] 查）。
    pub fn bind(port: u16, token: Option<String>) -> std::io::Result<Self> {
        let server = RpcServer::bind(port, token)?;
        log::info!("编辑器桥已就绪: {}", server.local_addr());
        Ok(Self { server })
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
            scripts: access.scripts,
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
