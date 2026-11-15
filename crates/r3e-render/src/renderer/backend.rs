use super::*;

/// 扩展四十四：把 `Renderer` 接到跨 API 契约上。
///
/// 只实现 [`RenderBackend`] 的四件事 —— 收口的正是「宿主与插件真正需要的
/// 跨后端信息」。调 wgpu 专有能力请直接用 `Renderer` 的固有方法；等第二个
/// 后端也有对应物时再往 trait 上提。
impl crate::gfx::RenderBackend for Renderer {
    fn gfx_api(&self) -> crate::gfx::GfxApi {
        self.gfx_info.api
    }

    fn gfx_info(&self) -> &crate::gfx::GfxInfo {
        &self.gfx_info
    }

    fn caps_line(&self) -> String {
        self.gfx_info.caps_line()
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn resize(&mut self, width: u32, height: u32) {
        // 显式点名固有方法，避免与 trait 方法同名时解析成递归调用
        Renderer::resize(self, width, height)
    }
}
