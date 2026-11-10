//! 屏幕射线拾取（扩展三十四）：把「鼠标点在哪个像素」变成「点到哪个物件」。
//!
//! 三段职责分离，每段都能单独测：
//!
//! 1. [`Ray`]：世界空间射线，方向归一化后 `t` 就是世界距离；
//! 2. [`Camera::screen_ray`] / [`Camera::ndc_ray`]：像素 / NDC → 世界射线（反投影近远平面点）；
//! 3. [`pick_nearest`] / [`pick_all`]：射线与节点的**世界 AABB** 求交，按距离排序。
//!
//! 为什么停在 AABB 而不到三角级：拾取可能每帧都在跑（悬停高亮、拖拽选中），
//! [`Aabb::ray_hit`] 是常数级成本；三角级精确拾取留给「点选已确定后要落点」这类
//! 低频场合。这里刻意复用与视锥剔除 / 阴影裁剪**同一个口径**（`mesh_aabb` 局部盒 ×
//! 节点世界矩阵），因此不会出现「画出来了却点不到」的分歧。
//!
//! 已知边界（写进文档，不当 bug）：
//!
//! * 蒙皮网格用**静止姿态**的局部盒，动画形变超出静止盒的部分点不到 ——
//!   与物理碰撞体推导用的是同一份盒子，两条路径的保守性一致；
//! * `mesh_aabb` 返回 `None` 的网格（提供者不供数据）不参与拾取，而不是命中原点；
//! * 命中判定不看材质透明与后处理，只按几何与可见性。

use std::collections::HashMap;

use glam::Vec3;

use crate::aabb::Aabb;
use crate::mesh::{MeshData, MeshProvider};
use crate::node::{MeshRef, Node, NodeId};
use crate::procedural::ProceduralProvider;
use crate::scene::Scene;

/// 父链上溯的防御上限：父子引用若被外部改成环，拾取不能死循环。
const MAX_PARENT_DEPTH: usize = 64;

/// 世界空间射线。`dir` 在构造时归一化，因此所有 `t` 都是世界距离。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    /// 构造射线。零向量 / 非有限方向退化为 `-Z`（右手系相机的朝向），
    /// 这样退化的鼠标坐标只会「点到相机正前方」，而不会产出 NaN 方向把整帧拾取污染。
    pub fn new(origin: Vec3, dir: Vec3) -> Self {
        let len = dir.length();
        let dir = if len > 1e-6 && len.is_finite() {
            dir / len
        } else {
            Vec3::NEG_Z
        };
        Self { origin, dir }
    }

    /// 射线上的点：`origin + dir * t`。
    pub fn point_at(&self, t: f32) -> Vec3 {
        self.origin + self.dir * t
    }
}

/// 一次拾取命中的结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickHit {
    /// 命中的节点。
    pub node: NodeId,
    /// 沿射线的命中距离（世界单位；起点已在该盒内时为 0）。
    pub distance: f32,
    /// 命中点世界坐标（射线进入包围盒的入口点）。
    pub point: Vec3,
    /// 命中节点本帧的世界包围盒，调用方拿它画高亮框 / 做二次精确判定。
    pub bounds: Aabb,
}

/// 纯数据版的网格包围盒表（实现 [`MeshProvider::mesh_aabb`]）。
///
/// 脚本侧没有渲染层的 `MeshCache`，但拾取需要「每个网格的局部盒」。这个表用来承接
/// 宿主同步过来的结果：资产导入 / 网格首次上传后登记一次，之后读表是纯查哈希。
/// 表里没有的键交给 [`PickBounds`] 的内置图元兜底。
#[derive(Debug, Default, Clone)]
pub struct MeshAabbCache {
    map: HashMap<MeshRef, Aabb>,
}

impl MeshAabbCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个网格的局部盒。空盒不登记（空盒拾取不到，登记了也只是噪声），返回是否写入。
    pub fn insert(&mut self, key: MeshRef, aabb: Aabb) -> bool {
        if aabb.is_empty() || !aabb.min.is_finite() || !aabb.max.is_finite() {
            return false;
        }
        self.map.insert(key, aabb);
        true
    }

    pub fn get(&self, key: MeshRef) -> Option<Aabb> {
        self.map.get(&key).copied()
    }

    pub fn contains(&self, key: MeshRef) -> bool {
        self.map.contains_key(&key)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }
}

impl MeshProvider for MeshAabbCache {
    /// 本表只管包围盒，不提供几何数据。
    fn mesh_data(&self, _key: MeshRef) -> Option<MeshData> {
        None
    }

    fn mesh_aabb(&self, key: MeshRef) -> Option<Aabb> {
        self.get(key)
    }
}

/// 「登记表优先、内置图元兜底」的拾取用提供者。
///
/// 组合顺序与渲染层的 `LayeredProvider`（资产 → 内置）相反但目的一致：
/// 资产网格的盒子来自宿主登记的 [`MeshAabbCache`]，内置 `cube/sphere/plane/cylinder`
/// 由 [`ProceduralProvider`] 现算（它们是常量几何，不值得占表位）。
pub struct PickBounds<'a> {
    cache: &'a MeshAabbCache,
}

impl<'a> PickBounds<'a> {
    pub fn new(cache: &'a MeshAabbCache) -> Self {
        Self { cache }
    }
}

impl MeshProvider for PickBounds<'_> {
    fn mesh_data(&self, _key: MeshRef) -> Option<MeshData> {
        None
    }

    fn mesh_aabb(&self, key: MeshRef) -> Option<Aabb> {
        self.cache
            .get(key)
            .or_else(|| ProceduralProvider.mesh_aabb(key))
    }
}

/// 命中全部与射线相交的节点，按距离**升序**返回（第一个就是最近命中）。
///
/// 排序用 `total_cmp`，NaN 不会 panic；等距时保持场景迭代顺序，结果稳定可测。
pub fn pick_all<P>(scene: &Scene, provider: &P, ray: &Ray) -> Vec<PickHit>
where
    P: MeshProvider + ?Sized,
{
    let mut hits = Vec::new();
    for node in scene.iter() {
        if let Some(hit) = hit_node(scene, provider, node, ray) {
            hits.push(hit);
        }
    }
    hits.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    hits
}

/// 最近的命中；没有任何相交返回 `None`。
///
/// 与 [`pick_all`] 分开实现而不是 `pick_all(...).into_iter().next()`：悬停高亮每帧都调，
/// 没必要为「只要最近一个」付整表排序的钱。
pub fn pick_nearest<P>(scene: &Scene, provider: &P, ray: &Ray) -> Option<PickHit>
where
    P: MeshProvider + ?Sized,
{
    let mut best: Option<PickHit> = None;
    for node in scene.iter() {
        let Some(hit) = hit_node(scene, provider, node, ray) else {
            continue;
        };
        let better = match &best {
            Some(b) => hit.distance < b.distance,
            None => true,
        };
        if better {
            best = Some(hit);
        }
    }
    best
}

/// 屏幕像素 → 射线 → 最近命中（`x`/`y` 为左上原点、y 向下的窗口像素坐标）。
pub fn pick_from_screen<P>(
    scene: &Scene,
    provider: &P,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> Option<PickHit>
where
    P: MeshProvider + ?Sized,
{
    let ray = scene.camera.screen_ray(x, y, width, height);
    pick_nearest(scene, provider, &ray)
}

/// 屏幕像素 → 射线 → 全部命中（距离升序），供「穿透选择」用。
pub fn pick_all_from_screen<P>(
    scene: &Scene,
    provider: &P,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> Vec<PickHit>
where
    P: MeshProvider + ?Sized,
{
    let ray = scene.camera.screen_ray(x, y, width, height);
    pick_all(scene, provider, &ray)
}

/// 单节点判定：可见性 → 局部盒 → 世界盒 → 射线相交。
fn hit_node<P>(scene: &Scene, provider: &P, node: &Node, ray: &Ray) -> Option<PickHit>
where
    P: MeshProvider + ?Sized,
{
    if !node_visible(scene, node) {
        return None;
    }
    let local = provider.mesh_aabb(node.mesh)?;
    if local.is_empty() {
        return None;
    }
    let world = local.transformed(&node.world);
    let distance = world.ray_hit(ray.origin, ray.dir)?;
    Some(PickHit {
        node: node.id,
        distance,
        point: ray.point_at(distance),
        bounds: world,
    })
}

/// 节点自身与全部祖先都 `visible` 才算可见（与渲染侧「父隐藏则子不画」一致）。
fn node_visible(scene: &Scene, node: &Node) -> bool {
    let mut current = Some(node);
    for _ in 0..MAX_PARENT_DEPTH {
        let Some(n) = current else {
            return true;
        };
        if !n.visible {
            return false;
        }
        current = n.parent.and_then(|id| scene.get(id));
    }
    // 父链超过防御上限：当作可见（fail-open），拾取比漏掉更安全
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Camera, ObjectDesc};

    fn desc(name: &str, position: Vec3) -> ObjectDesc {
        ObjectDesc {
            name: name.to_string(),
            position: Some([position.x, position.y, position.z]),
            ..ObjectDesc::default()
        }
    }

    fn scene_with(items: &[(&str, Vec3)]) -> Scene {
        let mut scene = Scene::new();
        for (name, position) in items {
            scene.spawn(desc(name, *position)).expect("spawn");
        }
        scene.update_world_transforms();
        scene
    }

    #[test]
    fn ray_new_normalizes_and_falls_back() {
        let ray = Ray::new(Vec3::ZERO, Vec3::new(0.0, 0.0, -4.0));
        assert!((ray.dir.length() - 1.0).abs() < 1e-6);
        assert!((ray.dir - Vec3::NEG_Z).length() < 1e-6);

        let degenerate = Ray::new(Vec3::ZERO, Vec3::ZERO);
        assert_eq!(degenerate.dir, Vec3::NEG_Z);
    }

    #[test]
    fn aabb_ray_hit_reports_entry_distance() {
        let box_ = Aabb::new(Vec3::splat(-0.5), Vec3::splat(0.5));
        let t = box_
            .ray_hit(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z)
            .expect("hit");
        assert!((t - 4.5).abs() < 1e-5, "t = {t}");

        // 起点在盒内：最近命中就是起点
        assert_eq!(
            box_.ray_hit(Vec3::ZERO, Vec3::NEG_Z),
            Some(0.0),
            "inside should be 0"
        );
        // 背向：不命中
        assert!(box_.ray_hit(Vec3::new(0.0, 0.0, 5.0), Vec3::Z).is_none());
        // 平行且偏离：不命中（走的是「无除法」分支，不应产生 NaN）
        assert!(box_
            .ray_hit(Vec3::new(3.0, 0.0, 5.0), Vec3::NEG_Z)
            .is_none());
        // 空盒永不命中
        assert!(Aabb::EMPTY.ray_hit(Vec3::ZERO, Vec3::NEG_Z).is_none());
    }

    #[test]
    fn camera_center_ray_points_along_view() {
        let camera = Camera::new(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO);
        let ray = camera.screen_ray(400.0, 300.0, 800.0, 600.0);
        // 屏幕中心 → 射线方向应指向 target
        let expect = (camera.target - camera.position).normalize();
        assert!(
            (ray.dir - expect).length() < 1e-3,
            "dir = {:?} expect = {expect:?}",
            ray.dir
        );
    }

    #[test]
    fn screen_ray_corners_span_frustum() {
        let camera = Camera::new(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO);
        let left = camera.screen_ray(0.0, 300.0, 800.0, 600.0).dir;
        let right = camera.screen_ray(800.0, 300.0, 800.0, 600.0).dir;
        let top = camera.screen_ray(400.0, 0.0, 800.0, 600.0).dir;
        // 左边界射线的 x 分量为负，右边界为正
        assert!(left.x < 0.0 && right.x > 0.0, "left/right 反向");
        // 屏幕上边（y = 0）对应世界 +Y（NDC y 向上）
        assert!(top.y > 0.0, "top 应朝上，实际 {:?}", top);
        // 左右镜像对称
        assert!((left.x + right.x).abs() < 1e-3);
    }

    #[test]
    fn pick_hits_nearest_cube_and_reports_point() {
        let mut scene = scene_with(&[
            ("near", Vec3::new(0.0, 0.0, 1.0)),
            ("far", Vec3::new(0.0, 0.0, -3.0)),
        ]);
        // 默认相机在 (4,3,6) 斜着看，这里换成「正对 Z 轴」以便断言坐标
        scene.camera = Camera::new(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO);
        let near_id = scene.find_id("near").expect("near");
        let ray = Ray::new(
            scene.camera.position,
            Vec3::new(0.0, 0.0, 1.0) - scene.camera.position,
        );

        let hit = pick_nearest(&scene, &ProceduralProvider, &ray).expect("hit");
        assert_eq!(hit.node, near_id, "应命中更近的 cube");
        // 世界盒边长 = 1（内置 cube），入口点在 z = 1.5，且正对射线上 x/y ≈ 0
        assert!((hit.point.z - 1.5).abs() < 1e-3, "point = {:?}", hit.point);
        assert!(hit.point.x.abs() < 1e-4 && hit.point.y.abs() < 1e-4);
        assert!(
            (hit.distance - 8.5).abs() < 1e-4,
            "distance = {}",
            hit.distance
        );

        let all = pick_all(&scene, &ProceduralProvider, &ray);
        assert_eq!(all.len(), 2, "两个 cube 都在射线上");
        assert!(all[0].distance <= all[1].distance, "必须按距离升序");
    }

    #[test]
    fn pick_from_screen_center_matches_camera_target() {
        let mut scene = scene_with(&[("target", Vec3::ZERO)]);
        scene.camera = Camera::new(Vec3::new(0.0, 0.0, 8.0), Vec3::ZERO);
        scene.camera.aspect = 800.0 / 600.0;
        let id = scene.find_id("target").expect("target");
        // 屏幕中心点中就应该是场景中心那个 cube
        let hit = pick_from_screen(&scene, &ProceduralProvider, 400.0, 300.0, 800.0, 600.0)
            .expect("中心必命中");
        assert_eq!(hit.node, id);
        // 角落（屏幕外沿）不该命中同一个物件
        let miss = pick_from_screen(&scene, &ProceduralProvider, 0.0, 0.0, 800.0, 600.0);
        assert!(miss.is_none(), "左上角不该命中场景中心的 cube");
    }

    #[test]
    fn invisible_node_and_hidden_parent_are_skipped() {
        let mut scene = scene_with(&[("a", Vec3::new(0.0, 0.0, 1.0))]);
        let ray = Ray::new(scene.camera.position, -scene.camera.position);

        // 自身不可见
        scene.find_mut("a").expect("a").visible = false;
        assert!(pick_nearest(&scene, &ProceduralProvider, &ray).is_none());

        // 父节点不可见：子节点也不该被点中
        scene.find_mut("a").expect("a").visible = true;
        let parent = scene
            .spawn(desc("parent", Vec3::ZERO))
            .expect("spawn parent");
        let child = scene.find_id("a").expect("a id");
        scene.get_mut(child).expect("a").parent = Some(parent);
        scene.find_mut("parent").expect("parent").visible = false;
        scene.update_world_transforms();
        assert!(pick_nearest(&scene, &ProceduralProvider, &ray).is_none());
    }

    #[test]
    fn missing_bounds_provider_yields_no_hit() {
        // 空缓存 + 不兜底内置图元：资产网格拿不到盒子就不该被点中
        let scene = scene_with(&[("a", Vec3::new(0.0, 0.0, 1.0))]);
        let ray = Ray::new(scene.camera.position, -scene.camera.position);
        let cache = MeshAabbCache::new();
        assert!(cache.is_empty());
        assert!(pick_nearest(&scene, &cache, &ray).is_none());

        // 登记过之后同一个射线就能命中，说明差异只来自盒子来源
        let mut cache = MeshAabbCache::new();
        let key = scene.find("a").expect("a").mesh;
        assert!(cache.insert(key, Aabb::new(Vec3::splat(-0.5), Vec3::splat(0.5))));
        assert!(pick_nearest(&scene, &cache, &ray).is_some());
        // 空盒不登记
        assert!(!cache.insert(key, Aabb::EMPTY));
    }
}
