//! `PhysicsHost` 的**实现**（M2-A03b）：把真实 `Physics` 门面适配成脚本侧能力面。
//!
//! 能力 trait 住在 `r3e-core::host`（公共契约层），本文件是唯一实现点：
//! 每条脚本物理 API 的「类型转换 + 门面调用」都集中在这个 `match` 里，
//! `r3e-script` 因此不必认识 `Physics` / `BodyDesc` / `Ray` 等本 crate 类型。
//!
//! 转换逻辑与旧 `api/physics.rs` 的闭包体逐字一致（M2 的零行为变化约束）。

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use glam::Vec3;
use r3e_core::host::{HitDto, PhysReq, PhysRsp, PhysicsHost};

use crate::{
    BodyDesc, BodyHandle, BodyKind, CastShape, Physics, PhysicsConfig, PhysicsKind, Ray, RayHit,
    ShapeCastHit,
};

/// 物理能力面门面：`ScriptHost` / 编辑器装配时用它把真门面递给脚本层。
///
/// 与真门面共享同一个 `Physics`（`Rc<RefCell<_>>`），脚本改的就是引擎那份状态。
pub struct PhysicsFacade {
    inner: Rc<RefCell<Physics>>,
}

impl PhysicsFacade {
    /// 包成能力面。返回 `Rc<dyn PhysicsHost>`，脚本侧只需要 trait。
    ///
    /// 名字刻意不叫 `new`：返回的不是 `Self`，`clippy::new_ret_no_self` 会拦。
    pub fn wrap(inner: Rc<RefCell<Physics>>) -> Rc<dyn PhysicsHost> {
        Rc::new(Self { inner })
    }

    /// 取回底层门面（装配层 / 测试要直接驱动它时用）。
    pub fn inner(&self) -> &Rc<RefCell<Physics>> {
        &self.inner
    }
}

fn ray_hit_dto(hit: RayHit) -> HitDto {
    HitDto {
        node: hit.node as i64,
        handle: hit.handle.0 as i64,
        distance: hit.distance as f64,
        point: [hit.point.x as f64, hit.point.y as f64, hit.point.z as f64],
        normal: [
            hit.normal.x as f64,
            hit.normal.y as f64,
            hit.normal.z as f64,
        ],
        inside: false,
    }
}

fn shape_hit_dto(hit: ShapeCastHit) -> HitDto {
    HitDto {
        node: hit.node as i64,
        handle: hit.handle.0 as i64,
        distance: hit.distance as f64,
        point: [hit.point.x as f64, hit.point.y as f64, hit.point.z as f64],
        normal: [
            hit.normal.x as f64,
            hit.normal.y as f64,
            hit.normal.z as f64,
        ],
        inside: hit.started_inside,
    }
}

fn vec3(v: [f64; 3]) -> Vec3 {
    Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32)
}

impl PhysicsHost for PhysicsFacade {
    fn execute(&self, req: PhysReq<'_>) -> PhysRsp {
        match req {
            PhysReq::Attach { node, kind, mass } => {
                // 与旧闭包逐字同序：先按节点推断描述，再覆盖脚本显式给的三项
                let mut desc = BodyDesc::from_node(node).unwrap_or_else(|| BodyDesc::infer(node));
                desc.kind = BodyKind::parse(kind);
                desc.mass = if mass > 0.0 { mass as f32 } else { 1.0 };
                desc.position = node.world_position();
                // 重复 attach 时先回收旧刚体，避免同一节点挂两个
                let old = node.body.map(BodyHandle);
                let mut p = self.inner.borrow_mut();
                if let Some(h) = old {
                    p.destroy_body(h);
                }
                match p.try_create_body(node.id, desc) {
                    Some(h) => PhysRsp::Int(h.0 as i64),
                    None => PhysRsp::Bool(false),
                }
            }
            PhysReq::Detach { handle } => {
                self.inner.borrow_mut().destroy_body(BodyHandle(handle));
                PhysRsp::Unit
            }
            PhysReq::Impulse { handle, v } => {
                self.inner
                    .borrow_mut()
                    .apply_impulse(BodyHandle(handle), vec3(v));
                PhysRsp::Unit
            }
            PhysReq::BodyCount => PhysRsp::Int(self.inner.borrow().body_count() as i64),
            PhysReq::Enabled => PhysRsp::Bool(self.inner.borrow().enabled()),
            PhysReq::SetEnabled(on) => {
                let mut p = self.inner.borrow_mut();
                p.set_enabled(on);
                PhysRsp::Bool(p.enabled())
            }
            PhysReq::Backend => PhysRsp::Text(self.inner.borrow().kind().name().to_string()),
            PhysReq::SetBackend(name) => {
                let Some(kind) = PhysicsKind::parse(name) else {
                    println!(
                        "[script] 未知物理后端 `{name}`（当前构建可用：{}）",
                        PhysicsKind::available_names().join(" / ")
                    );
                    return PhysRsp::Bool(false);
                };
                match self.inner.borrow_mut().set_kind(kind) {
                    Ok(()) => {
                        println!("[script] 物理后端 -> {}", kind.name());
                        PhysRsp::Bool(true)
                    }
                    Err(e) => {
                        println!("[script] {e}");
                        PhysRsp::Bool(false)
                    }
                }
            }
            PhysReq::Capabilities => PhysRsp::Text(self.inner.borrow().capabilities().summary()),
            PhysReq::HasCapability(key) => {
                PhysRsp::Bool(self.inner.borrow().capabilities().has(key))
            }
            PhysReq::Raycast { origin, dir, max } => {
                let ray = Ray::new(vec3(origin), vec3(dir));
                PhysRsp::Hit(
                    self.inner
                        .borrow()
                        .raycast(ray, max as f32)
                        .map(ray_hit_dto),
                )
            }
            PhysReq::ShapeCastSphere {
                center,
                radius,
                dir,
                max,
            } => PhysRsp::Hit(
                self.inner
                    .borrow()
                    .shape_cast(
                        CastShape::sphere(radius as f32),
                        vec3(center),
                        vec3(dir),
                        max as f32,
                    )
                    .map(shape_hit_dto),
            ),
            PhysReq::ShapeCastBox {
                center,
                half,
                dir,
                max,
            } => PhysRsp::Hit(
                self.inner
                    .borrow()
                    .shape_cast(
                        CastShape::boxed(vec3(half)),
                        vec3(center),
                        vec3(dir),
                        max as f32,
                    )
                    .map(shape_hit_dto),
            ),
            PhysReq::Overlap { center, radius } => PhysRsp::Nodes(
                self.inner
                    .borrow()
                    .overlap_sphere(vec3(center), radius as f32)
                    .iter()
                    .map(|h| h.node as i64)
                    .collect(),
            ),
            PhysReq::SetCcd(on) => {
                let mut physics = self.inner.borrow_mut();
                let supported = physics.set_ccd_default(on);
                let changed = physics.set_ccd_all(on);
                if !supported {
                    println!(
                        "[script] 当前物理后端不支持 CCD（能力位 ccd=false），physics_set_ccd 无效"
                    );
                    return PhysRsp::Bool(false);
                }
                println!(
                    "[script] CCD {}（已更新 {changed} 个刚体，缺省={}）",
                    if on { "已开启" } else { "已关闭" },
                    if physics.ccd_default() { "开" } else { "关" }
                );
                PhysRsp::Bool(true)
            }
            PhysReq::CcdDefault => PhysRsp::Bool(self.inner.borrow().ccd_default()),
            PhysReq::BodySetCcd { handle, on } => {
                PhysRsp::Bool(self.inner.borrow_mut().set_ccd(BodyHandle(handle), on))
            }
            PhysReq::BodyCcd { handle } => PhysRsp::Bool(
                self.inner
                    .borrow()
                    .ccd_of(BodyHandle(handle))
                    .unwrap_or(false),
            ),
            PhysReq::TakeEvents => PhysRsp::Texts(
                self.inner
                    .borrow_mut()
                    .drain_events()
                    .iter()
                    .map(|e| e.to_text())
                    .collect(),
            ),
            PhysReq::Gravity => PhysRsp::Float(self.inner.borrow().gravity.y as f64),
            PhysReq::SetGravity(g) => {
                self.inner.borrow_mut().set_gravity(vec3(g));
                PhysRsp::Unit
            }
            PhysReq::Ground => PhysRsp::Float(self.inner.borrow().ground() as f64),
            PhysReq::SetGround(y) => {
                self.inner.borrow_mut().set_ground(y as f32);
                PhysRsp::Unit
            }
            PhysReq::ClearBodies => {
                self.inner.borrow_mut().clear_bodies();
                PhysRsp::Unit
            }
            PhysReq::ResetScene { desc } => {
                let cfg = PhysicsConfig::from_scene(desc);
                let mut physics = self.inner.borrow_mut();
                physics.clear_bodies();
                PhysRsp::Err(physics.apply_config(cfg).err())
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
