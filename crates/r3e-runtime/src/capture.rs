//! 离屏截图：`--capture-frame` 与编辑器 `capture.frame` 共用的那条链路。
//!
//! 为什么住在 runtime 而不是编辑器宿主：无 RPC 的进程也要能截图。回归脚本、
//! CI 产图、`--capture-frame` 都是一条命令起一个进程，没人会为了拍一张基线图
//! 先把 JSON-RPC 桥点起来。反过来，编辑器宿主可以依赖 runtime 复用这段逻辑，
//! 依赖方向仍然只有 `editor-host → runtime` 一条。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use r3e_core::Scene;
use r3e_render::Renderer;

/// 一次成功截图的结果。
///
/// 字段全公开是刻意的：调用方（CLI 日志、`capture.frame` 的 RPC 响应）要原样
/// 回报「落在哪、多大、多少字节」，而不是再自己去 `metadata` 一遍——重新读一遍
/// 就可能读到被别人改过的文件，回报值就不再是「本次截图」的事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureInfo {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// 落盘后的实际字节数；恒 > 0（为 0 视为失败，见 `capture_to_file`）。
    pub bytes: u64,
    /// 读回时的目标纹理格式，排查偏色用（`capture.frame` 原样回报给客户端）。
    pub format: String,
}

impl CaptureInfo {
    /// 像素总字节数（RGBA8），用于区分「图写了但内容不全」和「图是黑的」。
    pub fn pixel_bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * 4
    }
}

/// 把相对路径按当前工作目录展开成绝对路径。
///
/// 截图是「写文件」操作，相对路径的基准必须是进程 cwd 而不是场景目录——
/// 用户在哪个目录敲的命令，产物就该出现在哪个目录，这是命令行工具的常识。
pub fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .map_err(|e| format!("无法解析相对路径的基准目录: {e}"))
}

/// `capture.frame` 不传 path 时的缺省文件名：毫秒时间戳，天然不撞名。
pub fn default_capture_name() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    format!("capture_{stamp}.png")
}

/// 离屏渲染一帧并落盘为 PNG。
///
/// 三步自检，缺一不可：渲染器必须给出一张非空位图、落盘后文件必须存在且非空、
/// 否则一律按失败上报。像素基线最怕的不是「拍失败」，而是「拍出一张 0 字节的
/// PNG 却当成通过」——之后所有回归都会拿这张坏图当基准。
pub fn capture_to_file(
    renderer: &mut Renderer,
    scene: &Scene,
    target: &Path,
) -> Result<CaptureInfo, String> {
    if target.is_dir() {
        return Err(format!("截图路径是目录而不是文件: {}", target.display()));
    }
    let captured = renderer.capture_frame(scene)?;
    if captured.rgba8.is_empty() {
        return Err(format!(
            "离屏渲染返回空位图（{}x{}），无法落盘",
            captured.width, captured.height
        ));
    }
    r3e_asset::write_png_rgba(target, captured.width, captured.height, &captured.rgba8)
        .map_err(|e| format!("截图落盘失败: {e}"))?;
    let bytes = std::fs::metadata(target)
        .map(|meta| meta.len())
        .map_err(|e| format!("截图落盘后读元数据失败: {e}"))?;
    if bytes == 0 {
        return Err(format!(
            "截图文件为空（{}），已按失败上报",
            target.display()
        ));
    }
    Ok(CaptureInfo {
        path: target.to_path_buf(),
        width: captured.width,
        height: captured.height,
        bytes,
        format: format!("{:?}", captured.format),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_path_keeps_absolute_untouched() {
        let abs = Path::new("C:/tmp/a.png");
        assert_eq!(absolute_path(abs).unwrap(), abs);
    }

    #[test]
    fn absolute_path_joins_cwd_for_relative() {
        let resolved = absolute_path(Path::new("shot.png")).unwrap();
        assert!(resolved.is_absolute(), "相对路径必须被展开成绝对路径");
        assert!(resolved.ends_with("shot.png"));
    }

    #[test]
    fn default_capture_name_has_png_suffix() {
        let name = default_capture_name();
        assert!(
            name.starts_with("capture_"),
            "缺省名要能一眼看出是截图: {name}"
        );
        assert!(name.ends_with(".png"));
    }

    #[test]
    fn pixel_bytes_counts_rgba8() {
        let info = CaptureInfo {
            path: PathBuf::from("a.png"),
            width: 640,
            height: 360,
            bytes: 1,
            format: "Rgba8UnormSrgb".to_string(),
        };
        assert_eq!(info.pixel_bytes(), 640 * 360 * 4);
    }
}
