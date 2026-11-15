//! 输出 / 生命周期 / 变换 / 外观 / PBR 材质。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「输出 / 生命周期 / 变换 / 外观 / PBR 材质」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---------------- 输出 ----------------
    engine.register_fn("log", |msg: &str| {
        println!("[script] {msg}");
    });

    // ---------------- 生命周期：生成 / 消失 ----------------
    let c = ctx.clone();
    engine.register_fn(
        "create_node",
        move |name: &str, mesh: &str, x: f64, y: f64, z: f64| -> bool {
            let desc = ObjectDesc {
                name: name.to_string(),
                mesh: Some(mesh.to_string()),
                position: Some([f32_of(x), f32_of(y), f32_of(z)]),
                ..Default::default()
            };
            match c.scene.borrow_mut().spawn(desc) {
                Ok(_) => true,
                Err(e) => {
                    println!("[script] create_node 失败: {e}");
                    false
                }
            }
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "create_node_at_origin",
        move |name: &str, mesh: &str| -> bool {
            let desc = ObjectDesc {
                name: name.to_string(),
                mesh: Some(mesh.to_string()),
                ..Default::default()
            };
            match c.scene.borrow_mut().spawn(desc) {
                Ok(_) => true,
                Err(e) => {
                    println!("[script] create_node 失败: {e}");
                    false
                }
            }
        },
    );

    // 用 JSON 描述物件，字段与场景文件一致
    let c = ctx.clone();
    engine.register_fn("create_node_json", move |json: &str| -> bool {
        match serde_json_lite(json) {
            Ok(desc) => match c.scene.borrow_mut().spawn(desc) {
                Ok(_) => true,
                Err(e) => {
                    println!("[script] create_node_json 失败: {e}");
                    false
                }
            },
            Err(e) => {
                println!("[script] create_node_json 解析失败: {e}");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("remove_node", move |name: &str| -> bool {
        let ok = c.scene.borrow_mut().despawn(name);
        if !ok {
            println!("[script] remove_node: 物件 `{name}` 不存在");
        }
        ok
    });

    let c = ctx.clone();
    engine.register_fn("remove_nodes", move |prefix: &str| -> i64 {
        let names: Vec<String> = c.scene.borrow().find_all(prefix);
        let mut s = c.scene.borrow_mut();
        let mut n = 0;
        for name in names {
            if s.despawn(&name) {
                n += 1;
            }
        }
        n
    });

    let c = ctx.clone();
    engine.register_fn("exists", move |name: &str| -> bool {
        c.scene.borrow().find(name).is_some()
    });

    let c = ctx.clone();
    engine.register_fn("count", move || -> i64 { c.scene.borrow().len() as i64 });

    let c = ctx.clone();
    engine.register_fn("names", move |prefix: &str| -> Array {
        c.scene
            .borrow()
            .find_all(prefix)
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });

    // ---------------- 变换 ----------------
    let c = ctx.clone();
    engine.register_fn(
        "set_pos",
        move |name: &str, x: f64, y: f64, z: f64| -> bool {
            edit(&c.scene, name, |n| {
                n.transform.position = Vec3::new(f32_of(x), f32_of(y), f32_of(z));
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "move_by",
        move |name: &str, dx: f64, dy: f64, dz: f64| -> bool {
            edit(&c.scene, name, |n| {
                n.transform.position += Vec3::new(f32_of(dx), f32_of(dy), f32_of(dz));
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("get_pos", move |name: &str| -> Array {
        let s = c.scene.borrow();
        match s.find(name) {
            Some(n) => vec![
                Dynamic::from(n.transform.position.x as f64),
                Dynamic::from(n.transform.position.y as f64),
                Dynamic::from(n.transform.position.z as f64),
            ],
            None => vec![Dynamic::from(0.0f64); 3],
        }
    });

    let c = ctx.clone();
    engine.register_fn(
        "set_rot",
        move |name: &str, x: f64, y: f64, z: f64| -> bool {
            edit(&c.scene, name, |n| {
                n.transform
                    .set_euler_deg(Vec3::new(f32_of(x), f32_of(y), f32_of(z)));
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "rotate",
        move |name: &str, x: f64, y: f64, z: f64| -> bool {
            edit(&c.scene, name, |n| {
                let e = n.transform.euler_deg() + Vec3::new(f32_of(x), f32_of(y), f32_of(z));
                n.transform.set_euler_deg(e);
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "set_scale",
        move |name: &str, x: f64, y: f64, z: f64| -> bool {
            edit(&c.scene, name, |n| {
                n.transform.scale = Vec3::new(f32_of(x), f32_of(y), f32_of(z));
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("set_scale_uniform", move |name: &str, s: f64| -> bool {
        edit(&c.scene, name, |n| {
            n.transform.scale = Vec3::splat(f32_of(s));
        })
    });

    // ---------------- 外观 ----------------
    let c = ctx.clone();
    engine.register_fn(
        "set_color",
        move |name: &str, r: f64, g: f64, b: f64| -> bool {
            edit(&c.scene, name, |n| {
                n.color = Vec4::new(f32_of(r), f32_of(g), f32_of(b), n.color.w);
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "set_color4",
        move |name: &str, r: f64, g: f64, b: f64, a: f64| -> bool {
            edit(&c.scene, name, |n| {
                n.color = Vec4::new(f32_of(r), f32_of(g), f32_of(b), f32_of(a));
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("set_visible", move |name: &str, v: bool| -> bool {
        edit(&c.scene, name, |n| n.visible = v)
    });

    // ---------------- PBR 材质 ----------------
    // 三个旋钮：metallic / roughness / emissive。base color 仍是物件颜色，
    // 语义不变。所有写入都走 `Scene::set_material*`，由 `SurfaceMaterial::sanitized`
    // 在**写入前**收敛（NaN / ±inf / 超界），脚本侧不做第二套夹取。
    let c = ctx.clone();
    engine.register_fn(
        "set_material",
        move |name: &str, metallic: f64, roughness: f64| -> bool {
            c.scene
                .borrow_mut()
                .set_material_params(name, f32_of(metallic), f32_of(roughness))
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "set_emissive",
        move |name: &str, r: f64, g: f64, b: f64| -> bool {
            c.scene
                .borrow_mut()
                .set_emissive(name, [f32_of(r), f32_of(g), f32_of(b)])
        },
    );

    let c = ctx.clone();
    engine.register_fn("clear_material", move |name: &str| -> bool {
        c.scene.borrow_mut().clear_material(name)
    });

    // 读回：收敛之后真正生效的材质快照；物件没有显式材质时给缺省值。
    let c = ctx.clone();
    engine.register_fn("material_of", move |name: &str| -> Map {
        let mut out = Map::new();
        let Some(material) = c.scene.borrow().material_of(name) else {
            out.insert("found".into(), Dynamic::from(false));
            return out;
        };
        out.insert("found".into(), Dynamic::from(true));
        out.insert("metallic".into(), Dynamic::from(material.metallic as f64));
        out.insert("roughness".into(), Dynamic::from(material.roughness as f64));
        out.insert(
            "emissive".into(),
            Dynamic::from(
                material
                    .emissive
                    .iter()
                    .map(|v| Dynamic::from(*v as f64))
                    .collect::<Array>(),
            ),
        );
        out
    });

    let c = ctx.clone();
    engine.register_fn("set_mesh", move |name: &str, mesh: &str| -> bool {
        match MeshRef::parse(mesh) {
            Some(kind) => edit(&c.scene, name, |n| n.mesh = kind),
            None => {
                println!("[script] 未知网格类型 `{mesh}`");
                false
            }
        }
    });
}
