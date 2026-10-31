//! 用 ai-ide `asset3d` 插件产出的 `.air.json` / `.air.bin` 校验引擎侧读取链路。
//!
//! 为什么单独做成 example：`.air.json` / `.air.bin` 不是靠扩展名分派的普通资产，
//! 只有 [`r3e_asset::load_air_file`] 这条「按内容嗅探」的入口能读它。把这一步
//! 跑通，才说明插件落盘的契约与 `r3e-asset` 的读端是同一份，而不是两套各说各话。
//!
//! 用法：
//!     cargo run -p r3e-asset --example air_probe -- a.air.json b.air.bin
//!
//! 退出码：全部读通 0；任一失败 1；参数不对 2。

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: air_probe <a.air.json|a.air.bin> [更多文件...]");
        std::process::exit(2);
    }

    let mut failed = 0usize;
    for arg in &args {
        let path = Path::new(arg);
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or(arg);
        match r3e_asset::load_air_file(path) {
            Ok(entry) => {
                let verts: usize = entry.meshes.iter().map(|m| m.positions.len()).sum();
                let tris: usize = entry.meshes.iter().map(|m| m.indices.len() / 3).sum();
                let uv = entry.meshes.first().map_or(0, |m| m.uvs.len());
                let (lo, hi) = entry
                    .meshes
                    .first()
                    .map_or(([0.0f32; 3], [0.0f32; 3]), |m| (m.aabb_min, m.aabb_max));
                println!(
                    "[ok  ] {name}: meshes={} verts={verts} tris={tris} uv={uv} \
                     materials={} textures={} nodes={} skins={} anims={}",
                    entry.meshes.len(),
                    entry.materials.len(),
                    entry.textures.len(),
                    entry.nodes.len(),
                    entry.skins.len(),
                    entry.animations.len(),
                );
                println!(
                    "       aabb_min=[{:.3}, {:.3}, {:.3}] aabb_max=[{:.3}, {:.3}, {:.3}]",
                    lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]
                );
                // 空资产能「读通」但没有意义：顶点全空说明字段映射在某处断掉了。
                if entry.meshes.is_empty() || verts == 0 || tris == 0 {
                    println!("[fail] {name}: 读通但几何为空，字段映射可疑");
                    failed += 1;
                }
            }
            Err(err) => {
                println!("[fail] {name}: {err}");
                failed += 1;
            }
        }
    }

    if failed > 0 {
        eprintln!("\n{failed} 个文件读取失败");
        std::process::exit(1);
    }
    println!("\n全部读通");
}
