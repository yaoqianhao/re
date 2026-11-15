//! 环境域：摄像机 / 环境探针 / 后处理 / 局部光源。
//!
//! 这几个字段在 `Scene` 上都是公开字段，写入就是一次赋值——命令层不额外做收敛，
//! 收敛责任留在 core（`sanitized()` 在构造侧已经做过，回写时再做一次是幂等的）。

use r3e_core::{Camera, EnvironmentSettings, LocalLight, PostFxSettings, Scene};

use crate::{command::CommandKind, DocError};

/// 写摄像机。
pub(super) fn write_camera(s: &mut Scene, camera: &Camera) -> Result<(), DocError> {
    s.camera = *camera;
    Ok(())
}

/// 写环境探针（`None` = 回退常数环境光）。
pub(super) fn write_environment(
    s: &mut Scene,
    env: &Option<EnvironmentSettings>,
) -> Result<(), DocError> {
    s.environment = *env;
    Ok(())
}

/// 写 HDR 后处理参数。
pub(super) fn write_postfx(s: &mut Scene, fx: &Option<PostFxSettings>) -> Result<(), DocError> {
    s.postfx = *fx;
    Ok(())
}

/// 按 key 写局部光源；同 key 覆盖，新 key 走容量检查。
///
/// 容量上限在 core 里（`MAX_LOCAL_LIGHTS`）：新增被拒时不报错也不落表，这里据「长度没变
/// 且此前不存在」判为 [`DocError::Rejected`]，避免命令显示成功而场景没变。
pub(super) fn write_local_light(s: &mut Scene, light: &LocalLight) -> Result<(), DocError> {
    let existed = s.lights.iter().any(|l| l.key == light.key);
    let before = s.lights.len();
    s.upsert_local_light(light.clone());
    if !existed && s.lights.len() == before {
        return Err(DocError::Rejected(format!(
            "局部光源表已满（{before} 盏），`{}` 未写入",
            light.key
        )));
    }
    Ok(())
}

/// 按 key 删局部光源。
pub(super) fn remove_local_light(s: &mut Scene, key: &str) -> Result<(), DocError> {
    if s.remove_local_light(key) {
        Ok(())
    } else {
        Err(DocError::NotFound(format!("局部光源 `{key}`")))
    }
}

impl CommandKind {
    /// 换摄像机命令。
    pub fn set_camera(s: &Scene, new: Camera) -> Self {
        Self::SetCamera { old: s.camera, new }
    }

    /// 换环境探针命令。
    pub fn set_environment(s: &Scene, new: Option<EnvironmentSettings>) -> Self {
        Self::SetEnvironment {
            old: s.environment,
            new,
        }
    }

    /// 换后处理命令。
    pub fn set_postfx(s: &Scene, new: Option<PostFxSettings>) -> Self {
        Self::SetPostFx { old: s.postfx, new }
    }

    /// 写局部光源命令（前像为同 key 的旧灯，没有则 `None`）。
    pub fn upsert_local_light(s: &Scene, next: LocalLight) -> Self {
        let old = s.lights.iter().find(|l| l.key == next.key).cloned();
        Self::UpsertLocalLight { next, old }
    }

    /// 删局部光源命令（前像为整盏灯）。
    pub fn remove_local_light(s: &Scene, key: &str) -> Result<Self, DocError> {
        let old = s
            .lights
            .iter()
            .find(|l| l.key == key)
            .cloned()
            .ok_or_else(|| DocError::NotFound(format!("局部光源 `{key}`")))?;
        Ok(Self::RemoveLocalLight { old })
    }
}
