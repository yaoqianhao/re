//! P3 探针：打印一个 FBX 的节点树与动画片段概况。
//!
//! 用法：`cargo run -p r3e-asset --example anim_probe -- <file.fbx> [更多文件...]`

use std::collections::HashMap;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: anim_probe <file.fbx> [更多文件...]");
        std::process::exit(2);
    }
    args.sort();
    for path in args {
        println!("==== {path}");
        match r3e_asset::import_fbx(std::path::Path::new(&path)) {
            Ok(entry) => report(&entry),
            Err(err) => println!("  导入失败: {err}"),
        }
    }
}

fn report(entry: &r3e_asset::AssetEntry) {
    println!(
        "  网格 {} / 节点 {} / 材质 {} / 贴图 {}",
        entry.meshes.len(),
        entry.nodes.len(),
        entry.materials.len(),
        entry.textures.len()
    );

    // 层级深度统计
    let mut depth: Vec<u32> = vec![0; entry.nodes.len()];
    for (i, node) in entry.nodes.iter().enumerate() {
        let mut d = 0;
        let mut cur = node.parent;
        while let Some(p) = cur {
            d += 1;
            cur = entry.nodes[p as usize].parent;
            if d > 32 {
                break;
            }
        }
        depth[i] = d;
    }
    let max_depth = depth.iter().copied().max().unwrap_or(0);
    println!("  最大层级深度: {max_depth}");

    let mut roots: Vec<&str> = entry
        .nodes
        .iter()
        .filter(|n| n.parent.is_none())
        .map(|n| n.path.as_str())
        .collect();
    roots.truncate(6);
    println!("  根节点: {:?}", roots);

    let mesh_nodes: Vec<&r3e_asset::NodeAsset> =
        entry.nodes.iter().filter(|n| n.mesh.is_some()).collect();
    println!("  挂网格的节点: {}", mesh_nodes.len());
    for n in mesh_nodes.iter().take(5) {
        println!(
            "    [{}] {} -> mesh#{}",
            n.index,
            n.path,
            n.mesh.unwrap_or(0)
        );
    }

    // 节点变换与网格自身 local matrix 的一致性抽样
    if let Some(m) = entry.meshes.first() {
        if let Some(n) = mesh_nodes.first() {
            let t = m.node_local_matrix;
            let approx = |a: f32, b: f64| (a as f64 - b).abs() < 1e-4;
            let consistent = approx(n.position[0], t[3][0] as f64)
                && approx(n.position[1], t[3][1] as f64)
                && approx(n.position[2], t[3][2] as f64);
            println!(
                "  节点 TRS 与网格 local matrix 平移一致: {consistent} (node={:?}, mat={:?})",
                n.position,
                [t[3][0], t[3][1], t[3][2]]
            );
        }
    }

    println!("  动画片段: {}", entry.animations.len());
    for clip in entry.animations.iter().take(8) {
        let keys: usize = clip.tracks.iter().map(|t| t.keys.len()).sum();
        let max_key = clip
            .tracks
            .iter()
            .map(|t| t.keys.len())
            .max()
            .unwrap_or(0);
        let t0 = clip
            .tracks
            .iter()
            .filter_map(|t| t.keys.first().map(|k| k.time))
            .fold(f32::INFINITY, f32::min);
        let t1 = clip
            .tracks
            .iter()
            .filter_map(|t| t.keys.last().map(|k| k.time))
            .fold(f32::NEG_INFINITY, f32::max);
        println!(
            "    `{}` 轨道 {} / 关键帧 {} / 最多单轨 {} / 时长 {:.2}s (t {:.2}..{:.2})",
            clip.name,
            clip.tracks.len(),
            keys,
            max_key,
            clip.duration,
            t0,
            t1
        );
        let names: Vec<&str> = clip
            .tracks
            .iter()
            .take(6)
            .map(|t| t.node_path.as_str())
            .collect();
        println!("      轨道样例: {:?}", names);
    }

    // 轨道 node_index 覆盖率
    if let Some(clip) = entry.animations.first() {
        let mut seen: HashMap<u32, usize> = HashMap::new();
        for t in &clip.tracks {
            *seen.entry(t.node_index).or_insert(0) += 1;
        }
        let dup = seen.values().filter(|v| **v > 1).count();
        println!(
            "    首段轨道去重后节点数 {} / 轨道数 {} (重复节点 {dup})",
            seen.len(),
            clip.tracks.len()
        );
    }
}
