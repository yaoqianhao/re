//! P4 探针：验证蒙皮矩阵口径。
//!
//! 对同一资产分别用两种「逆绑定」口径，在动画若干时刻对顶点做 CPU 蒙皮，
//! 与静态（bind 姿态烘焙）顶点比较最大/平均位移，位移越小说明口径与资产自洽。
//!
//! * `bind`：逆绑定 = `cluster.bind_to_world^-1`（FBX 规范口径）
//! * `rest`：逆绑定 = 骨骼节点默认世界矩阵的逆（以导入姿态为静止姿态）
//!
//! 用法：`cargo run -p r3e-asset --example skin_probe -- <file.fbx>`

use r3e_core::{Mat4, Vec3};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: skin_probe <file.fbx>");
        std::process::exit(2);
    }
    args.sort();
    for path in args {
        println!("==== {path}");
        match ufbx::load_file(&path, r3e_asset::load_opts()) {
            Ok(scene) => {
                report(&scene);
                validate(&scene);
            }
            Err(err) => println!("  加载失败: {:?} {}", err.type_, err.description),
        }
    }
}

fn mat(m: &ufbx::Matrix) -> Mat4 {
    r3e_asset::mat4_from(m)
}

fn node_name(node: &ufbx::Node) -> String {
    node.element.name.to_string()
}

fn report(scene: &ufbx::Scene) {
    for ni in 0..scene.nodes.len() {
        let node: &ufbx::Node = &scene.nodes[ni];
        let Some(mesh) = node.mesh.as_ref() else {
            continue;
        };
        println!(
            "  节点 `{}` 网格 `{}`：{} 顶点 / {} 三角 / skin_deformers {}",
            node_name(node),
            mesh.element.name,
            mesh.vertex_position.values.len(),
            mesh.num_triangles,
            mesh.skin_deformers.count
        );
        if mesh.skin_deformers.count == 0 {
            continue;
        }
        for di in 0..mesh.skin_deformers.count {
            let skin: &ufbx::SkinDeformer = &mesh.skin_deformers[di];
            println!(
                "    deformer #{di}: clusters {} / skin.vertices {} / weights {} / max_per_vertex {}",
                skin.clusters.count,
                skin.vertices.len(),
                skin.weights.len(),
                skin.max_weights_per_vertex
            );
            let mut hist = [0usize; 9];
            for vi in 0..skin.vertices.len() {
                let v: &ufbx::SkinVertex = &skin.vertices[vi];
                let mut n = 0usize;
                for k in 0..v.num_weights as usize {
                    let w: &ufbx::SkinWeight = &skin.weights[v.weight_begin as usize + k];
                    if w.weight > 1e-6 {
                        n += 1;
                    }
                }
                hist[n.min(8)] += 1;
            }
            println!(
                "      权重个数直方图: 1={} 2={} 3={} 4={} 8+={}",
                hist[1], hist[2], hist[3], hist[4], hist[8]
            );
        }
    }
    println!("  动画栈: {}", scene.anim_stacks.len());
}

/// 用某个时刻的骨骼世界矩阵做 CPU 蒙皮，返回（bind 口径位移, rest 口径位移）。
fn validate(scene: &ufbx::Scene) {
    for ni in 0..scene.nodes.len() {
        let node: &ufbx::Node = &scene.nodes[ni];
        let Some(mesh) = node.mesh.as_ref() else {
            continue;
        };
        if mesh.skin_deformers.count == 0 {
            continue;
        }
        let skin: &ufbx::SkinDeformer = &mesh.skin_deformers[0];
        let g2w = mat(&node.geometry_to_world);

        // 静态顶点（bind 姿态世界坐标）
        let mut statics: Vec<Vec3> = Vec::with_capacity(mesh.vertex_position.values.len());
        for i in 0..mesh.vertex_position.values.len() {
            let p = &mesh.vertex_position.values[i];
            statics.push(g2w.transform_point3(Vec3::new(p.x as f32, p.y as f32, p.z as f32)));
        }

        // 骨骼：element_id -> (bind_inv, rest_inv)
        let mut bind_inv: std::collections::HashMap<u32, Mat4> = Default::default();
        let mut rest_inv: std::collections::HashMap<u32, Mat4> = Default::default();
        for ci in 0..skin.clusters.count {
            let cluster: &ufbx::SkinCluster = &skin.clusters[ci];
            let Some(bone) = cluster.bone_node.as_ref() else {
                continue;
            };
            let id = bone.element.element_id;
            bind_inv.insert(id, mat(&cluster.bind_to_world).inverse());
            rest_inv.insert(id, mat(&bone.node_to_world).inverse());
        }

        for ai in 0..scene.anim_stacks.len() {
            let anim: &ufbx::Anim = scene.anim_stacks[ai].anim.as_ref();
            let name = scene.anim_stacks[ai].element.name.to_string();
            let times = [0.0f64, anim.time_end * 0.5];
            for t in times {
                let Ok(root) = ufbx::evaluate_scene(scene, anim, t, Default::default()) else {
                    println!("      动画 `{name}` 求值失败");
                    continue;
                };
                let eval_scene: &ufbx::Scene = &root;
                let mut now: std::collections::HashMap<u32, Mat4> = Default::default();
                for k in 0..eval_scene.nodes.len() {
                    let n: &ufbx::Node = &eval_scene.nodes[k];
                    now.insert(n.element.element_id, mat(&n.node_to_world));
                }

                let mut max_disp = [0.0f32; 2];
                let mut sum_disp = [0.0f64; 2];
                let mut moved = [0usize; 2];
                for vi in 0..skin.vertices.len().min(statics.len()) {
                    let v: &ufbx::SkinVertex = &skin.vertices[vi];
                    let p = statics[vi];
                    let mut acc = [Vec3::ZERO; 2];
                    for k in 0..v.num_weights as usize {
                        let w: &ufbx::SkinWeight = &skin.weights[v.weight_begin as usize + k];
                        if w.weight <= 0.0 {
                            continue;
                        }
                        let ci = w.cluster_index as usize;
                        let Some(bone) = skin.clusters[ci].bone_node.as_ref() else {
                            continue;
                        };
                        let id = bone.element.element_id;
                        let Some(b_now) = now.get(&id).copied() else {
                            continue;
                        };
                        if let Some(ib) = bind_inv.get(&id) {
                            acc[0] += (b_now * *ib).transform_point3(p) * w.weight as f32;
                        }
                        if let Some(ib) = rest_inv.get(&id) {
                            acc[1] += (b_now * *ib).transform_point3(p) * w.weight as f32;
                        }
                    }
                    for s in 0..2 {
                        let d = (acc[s] - p).length();
                        max_disp[s] = max_disp[s].max(d);
                        sum_disp[s] += d as f64;
                        if d > 1e-3 {
                            moved[s] += 1;
                        }
                    }
                }
                let n = skin.vertices.len().max(1) as f64;
                println!(
                    "    动画 `{name}` t={t:.2}s: bind 口径 max {:.4} avg {:.4} 移动 {}/{} | rest 口径 max {:.4} avg {:.4} 移动 {}/{}",
                    max_disp[0],
                    sum_disp[0] / n,
                    moved[0],
                    skin.vertices.len(),
                    max_disp[1],
                    sum_disp[1] / n,
                    moved[1],
                    skin.vertices.len()
                );
            }
        }
    }
}


