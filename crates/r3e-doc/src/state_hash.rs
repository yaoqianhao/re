//! 状态哈希：文档真值的唯一指纹（M0-S4 · P1）。
//!
//! 全仓只允许这一份实现。`r3e-replay` 的 headless 指纹、S5 的 invariants、M1 的
//! dry-run 返回值都调这里，不允许再写第二份 FNV。
//!
//! # 算法
//!
//! FNV-1a 64。不引密码学哈希：这里只做「同 / 不同」的二元判定（600 帧约 11 个采样点），
//! 不是防碰撞场景。所有输入走位表示（`f32::to_bits`、字符串按字节），不经过浮点格式化。
//!
//! # 规范化遍历顺序（顺序即契约，改动等于换哈希）
//!
//! ```text
//! 场景级（固定顺序）
//!   name → next_id → 节点数
//!   background → camera → light(含 shadow/cascade)
//!   environment(有无 + 内容) → postfx(有无 + 内容)
//!   lights + lights_enabled
//!   terrain（块数 + 每块内容）
//!   materials（份数 + 每份内容：名字 / 三标量 / 5 贴图槽 / uv 缩放）
//!   texts + text_enabled
//!   ui + ui_enabled + ui_above_text
//!   occlusion → environment_map(有无 + 内容)
//!   physics → assets → morphs
//! 节点（按 Scene::order，插入序，稳定）
//!   每节点：id → name → transform(9×f32) → visible → color → material(有无 + 内容)
//!           → material_ref(有无 + 名字) → parent(有无) → children(按序) → mesh(存在性 + 内容)
//! ```
//!
//! # 覆盖 / 不覆盖（评审对照表，新增 `Scene` 字段时先看这里）
//!
//! | 覆盖 | 理由 |
//! |---|---|
//! | `order` + 每节点 id/name/transform/visible/color/material/material_ref/parent/children/mesh | 命令 v0 的 17 条全部落在这几项上 |
//! | `next_id` | **必须覆盖**：不回退它，回放里同一脚本会分配出不同 `NodeId`，哈希假绿 |
//! | `name` / `background` / `camera` / `light` | `SetCamera` 直接改 |
//! | `environment` / `postfx` / `lights` / `texts` / `ui` / 三个 `*_enabled` / `ui_above_text` | `SetEnvironment` / `SetPostFx` / `Upsert*` / `Remove*` 直接改 |
//! | `occlusion` / `environment_map` / `physics` / `assets` / `morphs` | 文档语义的一部分，成本低 |
//! | `terrain`（M1-S3-C：名字 / 矩形 / 分辨率 / 材质 / 高度样本） | `paint_terrain` 直接改，AI 写入必须能被差分哈希看见 |
//! | `materials`（M1-S4-A：名字 / 三标量 / 5 贴图槽 / uv 缩放） | `UpsertMaterial` / `RemoveMaterial` 直接改，引用解析随之变化 |
//!
//! | 不覆盖 | 理由 |
//! |---|---|
//! | `time` / `frame` | 「世界节拍」，由 [`world_hash`] 单独负责；[`scene_hash`] 保持纯内容语义 |
//! | `anim_clips`（`HashMap<String, Arc<AnimClip>>`） | 运行期资产引用，非文档语义；`HashMap` 无序，纳入会让哈希随导入顺序漂移 |
//! | `animations` 播放进度 / `skins` 关节求解结果 | 每帧求值产物，进哈希等于给渲染层开门 |
//! | 物理求解缓存 | 同上 |
//! | `name_index` / `layout_version` | 派生索引与渲染缓存版本，不是文档内容 |
//! | `MorphInstance::slots`（`ShapeSlots`） | 字段私有，其上层 `weights` / `target_count` / `exclusive` 已覆盖语义 |
//!
//! 已知边界（M0-S4 §7 R7）：只保证「同机同二进制两次运行一致」，跨平台位级确定性推 M2。

use std::fmt;

use r3e_core::{
    AnimDecl, AssetDecl, BloomSettings, Camera, CascadeSettings, EnvironmentMapSettings,
    EnvironmentSettings, Light, LocalLight, LocalLightKind, MeshRef, MorphInstance, Node,
    OcclusionSettings, PhysicsSceneDesc, PostFxSettings, Scene, SceneMaterialDecl, ScreenText,
    ShadowSettings, SsaoSettings, SurfaceMaterial, TerrainPatch, TonemapMode, UiElement, UiKind,
    UiUnit, UiValue,
};

use crate::Document;

/// FNV-1a 64 的起始偏移。跨层复用（runtime 的 headless 指纹从同一初值起算）。
pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// 场景内容指纹（FNV-1a 64）。
///
/// `Display` 输出 16 位小写十六进制，与回放输出格式一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StateHash(u64);

impl StateHash {
    /// 直接由原始值构造（测试与跨层日志用）。
    pub const fn from_raw(v: u64) -> Self {
        Self(v)
    }

    /// 原始 64 位值。
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// 16 位小写十六进制，可直接打进日志。
    pub fn to_hex(self) -> String {
        format!("{self}")
    }
}

impl fmt::Display for StateHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

// ---------- 唯一混合原语 ----------

/// 混入一个 `u64`。
pub fn mix_u64(h: &mut u64, v: u64) {
    *h ^= v;
    *h = h.wrapping_mul(FNV_PRIME);
}

/// 混入一个 `f32`（走 `to_bits`，不看十进制表示）。
pub fn mix_f32(h: &mut u64, v: f32) {
    mix_u64(h, u64::from(v.to_bits()));
}

/// 混入一个字符串（先混字节长度，再逐字节）。
pub fn mix_str(h: &mut u64, s: &str) {
    mix_u64(h, s.len() as u64);
    for b in s.as_bytes() {
        mix_u64(h, u64::from(*b));
    }
}

// ---------- 内部小工具 ----------

fn mix_bool(h: &mut u64, v: bool) {
    mix_u64(h, u64::from(v));
}

fn mix_i32(h: &mut u64, v: i32) {
    mix_u64(h, v as u64);
}

fn mix_f32s(h: &mut u64, v: &[f32]) {
    mix_u64(h, v.len() as u64);
    for x in v {
        mix_f32(h, *x);
    }
}

fn mix_v3(h: &mut u64, v: r3e_core::Vec3) {
    mix_f32(h, v.x);
    mix_f32(h, v.y);
    mix_f32(h, v.z);
}

fn mix_v4(h: &mut u64, v: r3e_core::Vec4) {
    mix_f32(h, v.x);
    mix_f32(h, v.y);
    mix_f32(h, v.z);
    mix_f32(h, v.w);
}

fn mix_quat(h: &mut u64, q: r3e_core::Quat) {
    mix_f32(h, q.x);
    mix_f32(h, q.y);
    mix_f32(h, q.z);
    mix_f32(h, q.w);
}

fn mix_opt_str(h: &mut u64, v: Option<&str>) {
    match v {
        Some(s) => {
            mix_u64(h, 1);
            mix_str(h, s);
        }
        None => mix_u64(h, 0),
    }
}

fn mix_opt_f32(h: &mut u64, v: Option<f32>) {
    match v {
        Some(x) => {
            mix_u64(h, 1);
            mix_f32(h, x);
        }
        None => mix_u64(h, 0),
    }
}

fn mix_opt_bool(h: &mut u64, v: Option<bool>) {
    match v {
        Some(x) => {
            mix_u64(h, 1);
            mix_bool(h, x);
        }
        None => mix_u64(h, 0),
    }
}

fn mix_opt_v3(h: &mut u64, v: Option<[f32; 3]>) {
    match v {
        Some(a) => {
            mix_u64(h, 1);
            mix_f32s(h, &a);
        }
        None => mix_u64(h, 0),
    }
}

// ---------- 节点域 ----------

fn mix_material(h: &mut u64, m: &SurfaceMaterial) {
    mix_f32(h, m.metallic);
    mix_f32(h, m.roughness);
    mix_f32s(h, &m.emissive);
}

fn mix_mesh(h: &mut u64, m: &MeshRef) {
    match m {
        MeshRef::Builtin(b) => {
            mix_u64(h, 0);
            mix_u64(
                h,
                match b {
                    r3e_core::BuiltinMesh::Cube => 1,
                    r3e_core::BuiltinMesh::Sphere => 2,
                    r3e_core::BuiltinMesh::Plane => 3,
                    r3e_core::BuiltinMesh::Cylinder => 4,
                },
            );
        }
        MeshRef::Asset { asset, mesh } => {
            mix_u64(h, 1);
            mix_u64(h, u64::from(asset.value()));
            mix_u64(h, u64::from(*mesh));
        }
    }
}

fn mix_node(h: &mut u64, n: &Node) {
    mix_u64(h, n.id);
    mix_str(h, &n.name);
    mix_v3(h, n.transform.position);
    mix_quat(h, n.transform.rotation);
    mix_v3(h, n.transform.scale);
    mix_bool(h, n.visible);
    mix_v4(h, n.color);
    match &n.material {
        Some(m) => {
            mix_u64(h, 1);
            mix_material(h, m);
        }
        None => mix_u64(h, 0),
    }
    // 场景材质引用（M1-S4-A）：只混名字，不混解析结果——表内容由 `materials[]` 段单独覆盖。
    mix_opt_str(h, n.material_ref.as_deref());
    match n.parent {
        Some(p) => {
            mix_u64(h, 1);
            mix_u64(h, p);
        }
        None => mix_u64(h, 0),
    }
    mix_u64(h, n.children.len() as u64);
    for c in &n.children {
        mix_u64(h, *c);
    }
    mix_mesh(h, &n.mesh);
    // 语义标签：先混条数再逐条混，`["a","b"]` 与 `["ab"]` 不会撞车
    mix_u64(h, n.tags.len() as u64);
    for t in &n.tags {
        mix_str(h, t);
    }
    // 可交互半径：`None` 与 `Some(0.0)` 语义不同，用判别位区分
    match n.interact_radius {
        Some(r) => {
            mix_u64(h, 1);
            mix_f32(h, r);
        }
        None => mix_u64(h, 0),
    }
}

// ---------- 摄像机 / 灯光 / 环境 / 后处理 ----------

fn mix_camera(h: &mut u64, c: &Camera) {
    mix_v3(h, c.position);
    mix_v3(h, c.target);
    mix_v3(h, c.up);
    mix_f32(h, c.fov_deg);
    mix_f32(h, c.znear);
    mix_f32(h, c.zfar);
    mix_f32(h, c.aspect);
}

fn mix_cascade(h: &mut u64, c: &CascadeSettings) {
    mix_u64(h, u64::from(c.count));
    mix_f32(h, c.lambda);
    mix_f32(h, c.blend);
    mix_f32(h, c.max_distance);
    mix_f32(h, c.caster_extend);
    mix_bool(h, c.caster_cull);
}

fn mix_shadow(h: &mut u64, s: &ShadowSettings) {
    mix_bool(h, s.enabled);
    mix_u64(h, u64::from(s.size));
    mix_f32(h, s.ortho_size);
    mix_f32(h, s.bias);
    mix_f32(h, s.normal_bias);
    mix_f32(h, s.distance);
    mix_f32(h, s.strength);
    mix_cascade(h, &s.cascade);
}

fn mix_light(h: &mut u64, l: &Light) {
    mix_v3(h, l.direction);
    mix_v3(h, l.color);
    mix_f32(h, l.intensity);
    mix_v3(h, l.ambient);
    mix_shadow(h, &l.shadow);
}

fn mix_environment(h: &mut u64, e: &EnvironmentSettings) {
    mix_f32s(h, &e.sky_zenith);
    mix_f32s(h, &e.sky_horizon);
    mix_f32s(h, &e.ground);
    mix_f32s(h, &e.sun_dir);
    mix_f32s(h, &e.sun_color);
    mix_f32(h, e.sun_intensity);
    mix_f32(h, e.intensity);
}

fn mix_bloom(h: &mut u64, b: &BloomSettings) {
    mix_bool(h, b.enabled);
    mix_f32(h, b.threshold);
    mix_f32(h, b.knee);
    mix_f32(h, b.intensity);
}

fn mix_ssao(h: &mut u64, s: &SsaoSettings) {
    mix_bool(h, s.enabled);
    mix_f32(h, s.radius);
    mix_f32(h, s.bias);
    mix_f32(h, s.intensity);
    mix_u64(h, u64::from(s.samples));
}

fn mix_postfx(h: &mut u64, p: &PostFxSettings) {
    mix_f32(h, p.exposure);
    mix_u64(
        h,
        match p.tonemap {
            TonemapMode::None => 0,
            TonemapMode::Reinhard => 1,
            TonemapMode::Aces => 2,
        },
    );
    mix_bloom(h, &p.bloom);
    mix_bool(h, p.fxaa);
    mix_ssao(h, &p.ssao);
}

// ---------- 局部光 / 文本 / UI ----------

fn mix_local_light(h: &mut u64, l: &LocalLight) {
    mix_str(h, &l.key);
    mix_u64(
        h,
        match l.kind {
            LocalLightKind::Point => 0,
            LocalLightKind::Spot => 1,
        },
    );
    mix_f32s(h, &l.pos);
    mix_f32s(h, &l.color);
    mix_f32(h, l.intensity);
    mix_f32(h, l.range);
    mix_f32s(h, &l.dir);
    mix_f32(h, l.inner_deg);
    mix_f32(h, l.outer_deg);
    mix_bool(h, l.cast_shadow);
    mix_f32(h, l.shadow_bias);
}

fn mix_text(h: &mut u64, t: &ScreenText) {
    mix_str(h, &t.key);
    mix_str(h, &t.content);
    mix_f32(h, t.x);
    mix_f32(h, t.y);
    mix_f32(h, t.size);
    mix_f32s(h, &t.color);
    mix_u64(
        h,
        match t.align {
            r3e_core::TextAlign::Left => 0,
            r3e_core::TextAlign::Center => 1,
            r3e_core::TextAlign::Right => 2,
        },
    );
    mix_f32(h, t.wrap);
    mix_f32(h, t.line_height);
    mix_bool(h, t.visible);
}

fn mix_ui_value(h: &mut u64, v: &UiValue) {
    mix_f32(h, v.value);
    mix_u64(
        h,
        match v.unit {
            UiUnit::Px => 0,
            UiUnit::Percent => 1,
        },
    );
}

fn mix_ui(h: &mut u64, u: &UiElement) {
    mix_str(h, &u.key);
    mix_u64(
        h,
        match u.kind {
            UiKind::Rect => 0,
            UiKind::Border => 1,
            UiKind::Image => 2,
        },
    );
    mix_u64(
        h,
        match u.anchor {
            r3e_core::Anchor::TopLeft => 0,
            r3e_core::Anchor::TopCenter => 1,
            r3e_core::Anchor::TopRight => 2,
            r3e_core::Anchor::MiddleLeft => 3,
            r3e_core::Anchor::Center => 4,
            r3e_core::Anchor::MiddleRight => 5,
            r3e_core::Anchor::BottomLeft => 6,
            r3e_core::Anchor::BottomCenter => 7,
            r3e_core::Anchor::BottomRight => 8,
        },
    );
    for v in &u.offset {
        mix_ui_value(h, v);
    }
    for v in &u.size {
        mix_ui_value(h, v);
    }
    mix_f32s(h, &u.color);
    mix_f32(h, u.border_width);
    mix_str(h, &u.image);
    mix_f32s(h, &u.uv);
    for v in &u.slice_margin {
        mix_ui_value(h, v);
    }
    mix_i32(h, u.layer);
    mix_bool(h, u.visible);
}

// ---------- 场景级杂项 ----------

fn mix_occlusion(h: &mut u64, o: &OcclusionSettings) {
    mix_bool(h, o.enabled);
    mix_f32(h, o.depth_bias);
    mix_u64(h, u64::from(o.tile_size));
    mix_u64(h, u64::from(o.margin_px));
    mix_u64(h, u64::from(o.max_tiles));
}

fn mix_envmap(h: &mut u64, e: &EnvironmentMapSettings) {
    mix_bool(h, e.enabled);
    mix_opt_str(h, e.equirect.as_deref());
    match &e.faces {
        Some(faces) => {
            mix_u64(h, 1);
            mix_u64(h, faces.len() as u64);
            for f in faces {
                mix_str(h, f);
            }
        }
        None => mix_u64(h, 0),
    }
    mix_u64(h, e.face_size as u64);
    mix_u64(h, u64::from(e.mip_levels));
    mix_f32(h, e.intensity);
}

fn mix_physics(h: &mut u64, p: &PhysicsSceneDesc) {
    mix_opt_bool(h, p.enabled);
    mix_opt_str(h, p.backend.as_deref());
    mix_opt_v3(h, p.gravity);
    mix_opt_f32(h, p.ground);
    mix_opt_bool(h, p.ccd);
}

fn mix_anim_decl(h: &mut u64, a: &AnimDecl) {
    mix_opt_str(h, a.clip.as_deref());
    mix_opt_f32(h, a.speed);
    mix_opt_bool(h, a.looping);
    mix_opt_f32(h, a.time);
    mix_opt_bool(h, a.play);
}

fn mix_asset(h: &mut u64, a: &AssetDecl) {
    mix_str(h, &a.path);
    mix_opt_v3(h, a.position);
    mix_opt_v3(h, a.rotation);
    mix_opt_v3(h, a.scale);
    mix_opt_bool(h, a.spawn);
    match &a.anim {
        Some(anim) => {
            mix_u64(h, 1);
            mix_anim_decl(h, anim);
        }
        None => mix_u64(h, 0),
    }
}

fn mix_terrain(h: &mut u64, t: &TerrainPatch) {
    mix_str(h, &t.name);
    mix_f32s(h, &t.origin);
    mix_f32s(h, &t.size);
    mix_u64(h, u64::from(t.resolution[0]));
    mix_u64(h, u64::from(t.resolution[1]));
    mix_f32s(h, &t.albedo);
    mix_f32(h, t.roughness);
    // 高度样本逐点入哈希：`paint_terrain` 改一个顶点也必须让差分哈希看见。
    mix_u64(h, t.heights.len() as u64);
    mix_f32s(h, &t.heights);
}

/// 场景材质表项（M1-S4-A）：名字 + 三标量 + 5 个贴图槽 + uv 缩放。
///
/// 贴图槽按 [`TEXTURE_SLOTS`] 的固定顺序逐槽混，槽位顺序变了等于换哈希。
fn mix_scene_material(h: &mut u64, m: &SceneMaterialDecl) {
    mix_str(h, &m.name);
    mix_f32(h, m.metallic);
    mix_f32(h, m.roughness);
    mix_f32s(h, &m.emissive);
    for (_, path) in m.textures() {
        mix_str(h, path);
    }
    mix_f32s(h, &m.uv_scale);
}

fn mix_morph(h: &mut u64, m: &MorphInstance) {
    mix_u64(h, m.mesh_node);
    mix_u64(h, m.target_count as u64);
    for w in &m.weights {
        mix_f32(h, *w);
    }
    for g in &m.exclusive {
        mix_u64(h, u64::from(*g));
    }
}

// ---------- 入口 ----------

/// 场景内容指纹：不含 `time` / `frame`，命令改了内容才变。
pub fn scene_hash(scene: &Scene) -> StateHash {
    let mut h = FNV_OFFSET;

    // 场景级：固定顺序，与上面的契约注释一一对应。
    mix_str(&mut h, &scene.name);
    mix_u64(&mut h, scene.next_id());
    mix_u64(&mut h, scene.iter().count() as u64);
    mix_v4(&mut h, scene.background);
    mix_camera(&mut h, &scene.camera);
    mix_light(&mut h, &scene.light);
    match &scene.environment {
        Some(e) => {
            mix_u64(&mut h, 1);
            mix_environment(&mut h, e);
        }
        None => mix_u64(&mut h, 0),
    }
    match &scene.postfx {
        Some(p) => {
            mix_u64(&mut h, 1);
            mix_postfx(&mut h, p);
        }
        None => mix_u64(&mut h, 0),
    }
    mix_u64(&mut h, scene.lights.len() as u64);
    for l in &scene.lights {
        mix_local_light(&mut h, l);
    }
    mix_bool(&mut h, scene.lights_enabled);
    // 地形块表（M1-S3-C）：块数 + 每块内容，顺序即插入序（同名覆盖不改变位置）。
    mix_u64(&mut h, scene.terrain.len() as u64);
    for t in &scene.terrain {
        mix_terrain(&mut h, t);
    }
    // 场景材质表（M1-S4-A）：同名覆盖也是原地替换，顺序同样即插入序。
    mix_u64(&mut h, scene.materials.len() as u64);
    for m in &scene.materials {
        mix_scene_material(&mut h, m);
    }
    mix_u64(&mut h, scene.texts.len() as u64);
    for t in &scene.texts {
        mix_text(&mut h, t);
    }
    mix_bool(&mut h, scene.text_enabled);
    mix_u64(&mut h, scene.ui.len() as u64);
    for u in &scene.ui {
        mix_ui(&mut h, u);
    }
    mix_bool(&mut h, scene.ui_enabled);
    mix_bool(&mut h, scene.ui_above_text);
    mix_occlusion(&mut h, &scene.occlusion);
    match &scene.environment_map {
        Some(e) => {
            mix_u64(&mut h, 1);
            mix_envmap(&mut h, e);
        }
        None => mix_u64(&mut h, 0),
    }
    mix_physics(&mut h, &scene.physics);
    mix_u64(&mut h, scene.assets.len() as u64);
    for a in &scene.assets {
        mix_asset(&mut h, a);
    }
    mix_u64(&mut h, scene.morphs.len() as u64);
    for m in &scene.morphs {
        mix_morph(&mut h, m);
    }

    // 节点：按插入序，顺序稳定。
    for n in scene.iter() {
        mix_node(&mut h, n);
    }

    StateHash(h)
}

/// 世界指纹：内容 + 世界节拍（`time` / `frame`）。
///
/// 回放判定用这个：既能抓住内容分叉，也能抓住「节拍走偏一帧」。
pub fn world_hash(scene: &Scene) -> StateHash {
    let mut h = scene_hash(scene).raw();
    mix_f32(&mut h, scene.time);
    mix_u64(&mut h, scene.frame);
    StateHash(h)
}

impl Document {
    /// 当前内容的 [`StateHash`]。
    ///
    /// 以 `version` 作节拍器缓存：版本没变直接返回缓存值（版本是 S3 定下的唯一写入口
    /// 记账，宁可多跳一格、不会漏记，因此可以安全当缓存键）。纯读，不推进版本。
    pub fn state_hash(&self) -> StateHash {
        let version = self.version();
        if let Some((cached_version, cached)) = self.hash_cache.get() {
            if cached_version == version {
                return cached;
            }
        }
        let hash = scene_hash(self.as_scene());
        self.hash_cache.set(Some((version, hash)));
        hash
    }

    /// 当前内容的 [`StateHash`] + 世界节拍（不缓存：节拍每帧都动）。
    pub fn world_hash(&self) -> StateHash {
        world_hash(self.as_scene())
    }

    /// 清空哈希缓存。
    ///
    /// 事务专用（[`crate::transaction`]）：事务期间「版本不动但内容变了」，缓存键失效，
    /// 不清就会把中间态当成现值返回。不动版本、不置脏，纯缓存动作。
    pub(crate) fn invalidate_hash_cache(&mut self) {
        self.hash_cache.set(None);
    }
}

// 覆盖守卫测试见 `tests/state_hash.rs`（单文件行数门禁 + 只测公开 API）。
