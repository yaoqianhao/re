//! 顶点法线退化保护的静态闸门（扩展二十六）。
//!
//! `scale` 某轴为 0 时，主通道里的伴随矩阵整块归零，`normalize(0)` 在 GPU 上直接产
//! `NaN`，表现是物件整片刷黑或随机噪点；这类问题在无显示器环境根本跑不出来，
//! 因此把「保护分支必须存在」和「正常路径不许被顺手改写」都锁在源码文本上。
//!
//! 数值正确性由 `r3e-core` 的 `normal_matrix_safe` 覆盖（那颗是 CPU 参考实现），
//! 这里只保证 GPU 侧两条路径与它对齐。

const SHADER: &str = include_str!("../src/shader.wgsl");

/// 退化分支必须真的存在：长度判零 + 单位 Y 兜底。
#[test]
fn vertex_normal_has_degenerate_guard() {
    assert!(
        SHADER.contains("let raw_normal = normal_matrix * local_normal;"),
        "主通道法线没有先取原始向量——保护分支无从判断长度"
    );
    assert!(
        SHADER.contains("if length(raw_normal) > 1e-20 {"),
        "缺少长度判零：`normalize(0)` 在 GPU 上会产 NaN，整片像素会被刷黑"
    );
    assert!(
        SHADER.contains("out.world_normal = vec3<f32>(0.0, 1.0, 0.0);"),
        "缺少退化兜底方向（+Y）"
    );
}

/// 正常路径不许被改成除法：`normalize` 与 `v / length(v)` 存在 1ulp 差异，
/// 一旦换成除法，整批像素基线都会漂。
#[test]
fn healthy_path_still_uses_normalize() {
    assert!(
        SHADER.contains("out.world_normal = normalize(raw_normal);"),
        "正常路径必须保持 normalize(raw_normal)，否则视觉回归基线会无意义漂移"
    );
    assert!(
        !SHADER.contains("raw_normal / length(raw_normal)"),
        "正常路径被换成了手写除法——像素基线与 naga 实现细节绑死，不可取"
    );
}

/// 主通道里 `world_normal` 只允许一处写入（保护分支成对），
/// 免得日后有人补了兜底又在下游重新覆盖成裸 `normalize`。
#[test]
fn world_normal_written_once_per_branch() {
    let writes = SHADER.matches("out.world_normal =").count();
    assert_eq!(
        writes, 2,
        "主通路的 out.world_normal 应恰好两处写入（正常 + 退化兜底），实际 {writes} 处"
    );
}
