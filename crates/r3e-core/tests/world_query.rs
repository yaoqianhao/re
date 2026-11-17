//! WorldQuery 执行器的验收契约（M1-S2-A）。
//!
//! 计划里的验收口径是「四类过滤 + 投影 `{id,name,tags,pos,interact_radius}`」，所以这里
//! 不测「函数能跑通」，而是逐条钉住**语义**：
//!
//! 1. 类别判据：`interactable` / `prop` / `character` / `physical` 各自由什么决定；
//! 2. 标签：`all_tags` 是 AND、`any_tags` 是 OR，且**大小写敏感**（与写入侧一致）；
//! 3. 区域：按世界原点判定，含边界；
//! 4. 组件位：`require` 全具备、`exclude` 一票否决；
//! 5. 顺序与上限：插入顺序 + 尾部截断，不做抽样；
//! 6. 只读：查询前后场景 JSON 与 `layout_version` 必须一模一样。

use std::sync::Arc;

use r3e_core::anim::AnimBinding;
use r3e_core::{
    Aabb, AnimClip, AnimInstance, Component, MorphInstance, NodeId, NodeKind, ObjectDesc,
    PhysicsDesc, Query, Scene, SkinInstance, SurfaceMaterial, DEFAULT_LIMIT,
};

// ---------------------------------------------------------------------------
//夹具
// ---------------------------------------------------------------------------

fn physics() -> PhysicsDesc {
    PhysicsDesc {
        body: "static".to_string(),
        mass: 1.0,
        restitution: None,
        friction: None,
        collider: None,
        half_extents: None,
        ccd: None,
    }
}

fn desc(name: &str) -> ObjectDesc {
    ObjectDesc {
        name: name.to_string(),
        ..ObjectDesc::default()
    }
}

fn names(views: &[r3e_core::NodeView]) -> Vec<&str> {
    views.iter().map(|v| v.name.as_str()).collect()
}

/// 八节点夹具，插入顺序即期望的返回顺序。
///
/// | 名字 | 位置 | 语义 |
/// |---|---|---|
/// | floor | 0,0,0 | 普通布景 |
/// | door | 0,0,2 | 可交互（半径 1.5，标签 door） |
/// | coin | 3,0,0 | 可交互（半径 0.5，标签 pickup/coin） |
/// | crate | -3,0,0 | 物理刚体（标签 crate） |
/// | torch | 0,2,0 | 带材质（标签 light） |
/// | hero | 0,0,-5 | 角色：蒙皮 + 形变 + 动画（标签 character/enemy） |
/// | blade | 0,1,0 | hero 的子节点 |
/// | gizmo | 1.5,1,0 | 不可见（标签 debug） |
fn base_scene() -> Scene {
    let mut scene = Scene::new();

    scene.spawn(desc("floor")).unwrap();
    scene
        .spawn(ObjectDesc {
            tags: Some(vec!["door".to_string()]),
            interact_radius: Some(1.5),
            position: Some([0.0, 0.0, 2.0]),
            ..desc("door")
        })
        .unwrap();
    scene
        .spawn(ObjectDesc {
            tags: Some(vec!["pickup".to_string(), "coin".to_string()]),
            interact_radius: Some(0.5),
            position: Some([3.0, 0.0, 0.0]),
            ..desc("coin")
        })
        .unwrap();
    scene
        .spawn(ObjectDesc {
            tags: Some(vec!["crate".to_string()]),
            physics: Some(physics()),
            position: Some([-3.0, 0.0, 0.0]),
            ..desc("crate")
        })
        .unwrap();
    scene
        .spawn(ObjectDesc {
            tags: Some(vec!["light".to_string()]),
            material: Some(SurfaceMaterial::new(0.0, 0.6)),
            position: Some([0.0, 2.0, 0.0]),
            ..desc("torch")
        })
        .unwrap();
    scene
        .spawn(ObjectDesc {
            tags: Some(vec!["character".to_string(), "enemy".to_string()]),
            position: Some([0.0, 0.0, -5.0]),
            ..desc("hero")
        })
        .unwrap();
    scene
        .spawn(ObjectDesc {
            parent: Some("hero".to_string()),
            position: Some([0.0, 1.0, 0.0]),
            ..desc("blade")
        })
        .unwrap();
    scene
        .spawn(ObjectDesc {
            tags: Some(vec!["debug".to_string()]),
            visible: Some(false),
            position: Some([1.5, 1.0, 0.0]),
            ..desc("gizmo")
        })
        .unwrap();

    // hero 同时是蒙皮 / 形变 / 动画的载体
    let mesh = scene.find_id("hero").unwrap();
    scene.add_skin(SkinInstance::new("hero_skin", mesh));
    scene.add_morph(MorphInstance::new(mesh, 3));
    let mut anim = AnimInstance::new("hero_idle", Arc::new(AnimClip::default()));
    anim.bindings.push(AnimBinding {
        node: mesh,
        node_index: 0,
        path: "root".to_string(),
        track: 0,
    });
    scene.add_anim_instance(anim);

    scene.update_world_transforms();
    scene
}

// ---------------------------------------------------------------------------
// 1. 类别判据
// ---------------------------------------------------------------------------

#[test]
fn any_returns_every_node_in_insertion_order() {
    let scene = base_scene();
    let all = scene.query(&Query::new(NodeKind::Any));
    assert_eq!(
        vec!["floor", "door", "coin", "crate", "torch", "hero", "blade", "gizmo"],
        names(&all),
        "Any 应返回全部节点且保持插入顺序"
    );
    assert_eq!(scene.len(), all.len());
}

#[test]
fn interactable_requires_declared_radius() {
    let scene = base_scene();
    let hits = scene.query(&Query::new(NodeKind::Interactable));
    assert_eq!(vec!["door", "coin"], names(&hits));
}

#[test]
fn prop_is_the_visible_leftover() {
    let scene = base_scene();
    let hits = scene.query(&Query::new(NodeKind::Prop));
    assert_eq!(
        vec!["floor", "torch", "blade"],
        names(&hits),
        "Prop = 可见 且 无交互 / 无物理 / 无动画 / 无蒙皮 / 无形变；gizmo 不可见应被排除"
    );
}

#[test]
fn character_needs_skinning_or_morphing() {
    let scene = base_scene();
    assert_eq!(
        vec!["hero"],
        names(&scene.query(&Query::new(NodeKind::Character)))
    );
    assert_eq!(
        vec!["hero"],
        names(&scene.query(&Query::new(NodeKind::Skinned)))
    );
    assert_eq!(
        vec!["hero"],
        names(&scene.query(&Query::new(NodeKind::Morphing)))
    );
}

#[test]
fn physical_needs_physics_desc_or_body_handle() {
    let mut scene = base_scene();
    assert_eq!(
        vec!["crate"],
        names(&scene.query(&Query::new(NodeKind::Physical)))
    );

    // 后端挂上刚体句柄也算物理物件（physics 描述可以为空）
    let id = scene.find_id("torch").unwrap();
    scene.get_mut(id).unwrap().body = Some(7);
    assert_eq!(
        vec!["crate", "torch"],
        names(&scene.query(&Query::new(NodeKind::Physical)))
    );
}

#[test]
fn animated_follows_anim_bindings() {
    let mut scene = base_scene();
    assert_eq!(
        vec!["hero"],
        names(&scene.query(&Query::new(NodeKind::Animated)))
    );

    // 解绑后不再命中：判据是「当前绑定」，不是「曾经绑定过」
    scene
        .anim_instance_mut("hero_idle")
        .expect("实例应已登记")
        .bindings
        .clear();
    assert!(
        scene.query(&Query::new(NodeKind::Animated)).is_empty(),
        "没有绑定的动画实例不该让任何节点变成 animated"
    );
    // 蒙皮判据独立于动画：解绑动画不影响 character
    assert_eq!(
        vec!["hero"],
        names(&scene.query(&Query::new(NodeKind::Character)))
    );
}

// ---------------------------------------------------------------------------
// 2. 标签
// ---------------------------------------------------------------------------

#[test]
fn all_tags_is_and_semantics() {
    let scene = base_scene();
    assert_eq!(
        vec!["hero"],
        names(
            &scene.query(
                &Query::new(NodeKind::Any)
                    .with_tag("character")
                    .with_tag("enemy")
            )
        )
    );
    let none = scene.query(
        &Query::new(NodeKind::Any)
            .with_tag("character")
            .with_tag("pickup"),
    );
    assert!(none.is_empty(), "多标签同时要求时是 AND，不命中即空");
}

#[test]
fn any_tags_is_or_semantics() {
    let scene = base_scene();
    let hits = scene.query(
        &Query::new(NodeKind::Any)
            .with_any_tag("door")
            .with_any_tag("light"),
    );
    assert_eq!(vec!["door", "torch"], names(&hits));
}

#[test]
fn tags_match_literally_and_trimmed() {
    let scene = base_scene();
    assert_eq!(
        vec!["door"],
        names(&scene.query(&Query::new(NodeKind::Any).with_tag("  door  "))),
        "查询侧容忍首尾空白"
    );
    assert!(
        scene
            .query(&Query::new(NodeKind::Any).with_tag("Door"))
            .is_empty(),
        "大小写不折叠：Door 与 door 是两个标签（与 normalize_tags 的契约一致）"
    );
    assert!(
        scene
            .query(&Query::new(NodeKind::Any).with_tag("   "))
            .is_empty(),
        "空白标签不该被当作通配符命中全部"
    );
}

// ---------------------------------------------------------------------------
// 3. 区域
// ---------------------------------------------------------------------------

#[test]
fn region_filters_by_world_origin_with_inclusive_bounds() {
    let scene = base_scene();
    let region = Aabb {
        min: r3e_core::Vec3::new(-1.0, 0.0, -6.0),
        max: r3e_core::Vec3::new(1.0, 2.5, 2.0),
    };
    let hits = scene.query(&Query::new(NodeKind::Any).in_region(region));
    assert_eq!(
        vec!["floor", "door", "torch", "hero", "blade"],
        names(&hits),
        "只按世界原点判定：coin(3,0,0) 与 crate(-3,0,0) 在区域外"
    );

    // 边界点算命中
    let edge = Aabb {
        min: r3e_core::Vec3::new(3.0, 0.0, 0.0),
        max: r3e_core::Vec3::new(3.0, 0.0, 0.0),
    };
    assert_eq!(
        vec!["coin"],
        names(&scene.query(&Query::new(NodeKind::Any).in_region(edge))),
        "落在边界上的物件应命中"
    );
}

#[test]
fn region_composes_with_kind() {
    let scene = base_scene();
    let region = Aabb {
        min: r3e_core::Vec3::new(-1.0, -1.0, -6.0),
        max: r3e_core::Vec3::new(1.0, 3.0, 3.0),
    };
    let hits = scene.query(&Query::new(NodeKind::Interactable).in_region(region));
    assert_eq!(vec!["door"], names(&hits), "coin 在区域外");
}

// ---------------------------------------------------------------------------
// 4. 组件位
// ---------------------------------------------------------------------------

#[test]
fn require_needs_every_component() {
    let scene = base_scene();
    let hits = scene.query(
        &Query::new(NodeKind::Any)
            .requiring(Component::Material)
            .requiring(Component::Tagged),
    );
    assert_eq!(vec!["torch"], names(&hits));
}

#[test]
fn exclude_vetoes_any_component() {
    let scene = base_scene();
    let hits = scene.query(
        &Query::new(NodeKind::Any)
            .requiring(Component::Tagged)
            .excluding(Component::Interact),
    );
    assert_eq!(
        vec!["crate", "torch", "hero", "gizmo"],
        names(&hits),
        "有交互能力的 door / coin 应被排除"
    );
}

#[test]
fn parent_and_child_components() {
    let scene = base_scene();
    assert_eq!(
        vec!["blade"],
        names(&scene.query(&Query::new(NodeKind::Any).requiring(Component::Parent)))
    );
    assert_eq!(
        vec!["hero"],
        names(&scene.query(&Query::new(NodeKind::Any).requiring(Component::Child)))
    );
}

// ---------------------------------------------------------------------------
// 5. 顺序、上限、投影
// ---------------------------------------------------------------------------

#[test]
fn limit_truncates_tail_keeping_insertion_order() {
    let scene = base_scene();
    let hits = scene.query(&Query::new(NodeKind::Any).with_limit(3));
    assert_eq!(vec!["floor", "door", "coin"], names(&hits));

    // limit = 0 表示不限
    assert_eq!(scene.len(), scene.query(&Query::new(NodeKind::Any)).len());
    assert_eq!(DEFAULT_LIMIT, 64, "协议层缺省值不该被随手改动");
}

#[test]
fn view_projects_exactly_the_five_fields() {
    let scene = base_scene();
    let hits = scene.query(&Query::new(NodeKind::Interactable).with_tag("door"));
    let view = &hits[0];
    let node = scene.get(view.id).unwrap();

    assert_eq!(node.id, view.id);
    assert_eq!("door", view.name);
    assert_eq!(vec!["door".to_string()], view.tags);
    assert_eq!(Some(1.5), view.interact_radius);
    assert_eq!([0.0, 0.0, 2.0], view.position);

    let wp = node.world_position();
    assert_eq!([wp.x, wp.y, wp.z], view.position, "位置取世界空间");
}

#[test]
fn view_position_is_world_space_for_children() {
    let scene = base_scene();
    let blade = scene.find_id("blade").unwrap();
    let view = scene
        .query(&Query::new(NodeKind::Any).with_limit(0))
        .into_iter()
        .find(|v| v.id == blade)
        .expect("blade 应能被查到");
    assert_eq!(
        [0.0, 1.0, -5.0],
        view.position,
        "子节点位置应是父节点(-5) + 本地偏移(1)，而不是本地 (0,1,0)"
    );
}

// ---------------------------------------------------------------------------
// 6. 只读 + 确定性 + stats 一致性
// ---------------------------------------------------------------------------

#[test]
fn query_is_read_only() {
    let scene = base_scene();
    let before_json = scene.to_json_pretty().unwrap();
    let before_version = scene.layout_version();

    let _ = scene.query(
        &Query::new(NodeKind::Interactable)
            .with_tag("door")
            .in_region(Aabb {
                min: r3e_core::Vec3::splat(-100.0),
                max: r3e_core::Vec3::splat(100.0),
            })
            .requiring(Component::Interact)
            .with_limit(10),
    );
    let _ = scene.query_stats();

    assert_eq!(
        before_json,
        scene.to_json_pretty().unwrap(),
        "查询不得改动场景"
    );
    assert_eq!(
        before_version,
        scene.layout_version(),
        "查询不得触碰布局版本"
    );
}

#[test]
fn repeated_queries_are_identical() {
    let scene = base_scene();
    let q = Query::new(NodeKind::Any)
        .with_any_tag("door")
        .with_any_tag("character");
    let a = scene.query(&q);
    let b = scene.query(&q);
    assert_eq!(a, b, "同一场景同一查询应逐字段一致");
}

#[test]
fn stats_agree_with_query() {
    let scene = base_scene();
    let stats = scene.query_stats();
    assert_eq!(scene.len(), stats.nodes);
    assert_eq!(7, stats.visible, "只有 gizmo 不可见");
    assert_eq!(
        scene.query(&Query::new(NodeKind::Interactable)).len(),
        stats.interactable
    );
    assert_eq!(
        scene
            .query(&Query::new(NodeKind::Any).requiring(Component::Tagged))
            .len(),
        stats.tagged
    );
    assert_eq!(Some(&1), stats.tags.get("door"), "标签表应统计出现次数");
    for kind in NodeKind::ALL {
        let expected = scene.query(&Query::new(kind)).len();
        let got = stats.by_kind.get(&kind).copied().unwrap_or(0);
        assert_eq!(
            expected,
            got,
            "类别 {} 的计数应与 query 结果一致",
            kind.as_str()
        );
    }
}

// ---------------------------------------------------------------------------
// 7. 枚举的字符串面
// ---------------------------------------------------------------------------

#[test]
fn node_kind_roundtrips_through_str() {
    assert_eq!(8, NodeKind::ALL.len(), "改枚举时请同步 ALL 与本断言");
    for kind in NodeKind::ALL {
        assert_eq!(
            Some(kind),
            NodeKind::parse(kind.as_str()),
            "as_str / parse 应互逆：{}",
            kind.as_str()
        );
    }
    assert_eq!(Some(NodeKind::Any), NodeKind::parse(""));
    assert_eq!(Some(NodeKind::Any), NodeKind::parse("ANY"));
    assert_eq!(Some(NodeKind::Interactable), NodeKind::parse("interact"));
    assert_eq!(None, NodeKind::parse("bogus"), "未知类别应报错而不是猜一个");
}

#[test]
fn component_roundtrips_through_str() {
    assert_eq!(9, Component::ALL.len(), "改枚举时请同步 ALL 与本断言");
    for component in Component::ALL {
        assert_eq!(Some(component), Component::parse(component.as_str()));
    }
    assert_eq!(None, Component::parse("bogus"));
}

#[test]
fn query_builder_defaults_are_open() {
    let q = Query::default();
    assert_eq!(NodeKind::Any, q.kind);
    assert!(q.all_tags.is_empty());
    assert!(q.any_tags.is_empty());
    assert!(q.region.is_none());
    assert!(q.require.is_empty() && q.exclude.is_empty());
    assert_eq!(0, q.limit, "默认不限条数，缺省上限由协议层给 DEFAULT_LIMIT");

    let scene = base_scene();
    assert_eq!(scene.len(), scene.query(&q).len());
}

#[test]
fn node_ids_are_stable_across_queries() {
    let scene = base_scene();
    let by_name: Vec<(NodeId, String)> = scene
        .query(&Query::new(NodeKind::Any))
        .into_iter()
        .map(|v| (v.id, v.name))
        .collect();
    for (id, name) in by_name {
        assert_eq!(Some(id), scene.find_id(&name));
    }
}
