//! 场景级遮挡剔除开关与环境贴图探针。

use super::*;

impl Scene {
    /// 遮挡剔除总开关（脚本 `set_occlusion_enabled`）。
    ///
    /// 关掉后渲染层整帧跳过遮挡判定：不读回深度、不建金字塔，提交序列退化成
    /// 「只有视锥剔除」——也就是加这个功能之前的样子。参数（偏置 / tile 等）保留，
    /// 再打开即按原参数恢复。返回切换后的状态。
    pub fn set_occlusion_enabled(&mut self, on: bool) -> bool {
        self.occlusion.enabled = on;
        self.occlusion.enabled
    }

    /// 当前是否启用遮挡剔除。
    pub fn occlusion_enabled(&self) -> bool {
        self.occlusion.enabled
    }

    /// 覆盖式写入整段遮挡剔除参数（脚本 / RPC 用），写入前统一收敛。
    pub fn set_occlusion(&mut self, settings: OcclusionSettings) -> OcclusionSettings {
        self.occlusion = settings.sanitized();
        self.occlusion
    }

    // ---------- HDR 立方体反射探针（扩展二十三） ----------

    /// 覆盖式写入整段探针配置，写入前统一收敛（脚本 / RPC / 场景 JSON 共用入口）。
    ///
    /// 返回写入后的配置；传入的路径全是空白时配置仍在，只是
    /// [`EnvironmentMapSettings::is_active`] 为 false，渲染层退回解析 IBL。
    pub fn set_environment_map(
        &mut self,
        settings: EnvironmentMapSettings,
    ) -> EnvironmentMapSettings {
        let settings = settings.sanitized();
        self.environment_map = Some(settings.clone());
        settings
    }

    /// 探针总开关（脚本 `set_env_map_enabled`）。关掉后路径保留，渲染层退回解析 IBL。
    pub fn set_environment_map_enabled(&mut self, on: bool) -> bool {
        let mut settings = self.environment_map.clone().unwrap_or_default();
        settings.enabled = on;
        self.environment_map = Some(settings.sanitized());
        on
    }

    /// 当前是否有**生效中**的 HDR 探针（开关打开且来源有效）。
    pub fn environment_map_active(&self) -> bool {
        self.environment_map
            .as_ref()
            .map(EnvironmentMapSettings::is_active)
            .unwrap_or(false)
    }

    /// 移除探针配置，彻底退回解析 IBL（脚本 `clear_env_map`）。
    pub fn clear_environment_map(&mut self) -> bool {
        self.environment_map.take().is_some()
    }

    /// 来源标签（`none` / `disabled` / `equirect` / `faces`），供日志与编辑器面板显示。
    pub fn env_map_source(&self) -> String {
        self.environment_map
            .as_ref()
            .map(EnvironmentMapSettings::source_label)
            .unwrap_or("none")
            .to_string()
    }

    // ---------- 生命周期 ----------
}
