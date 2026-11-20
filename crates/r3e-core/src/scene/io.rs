//! 场景文件的加载、保存与往返投影（含节点描述回写）。

use super::*;

impl Scene {
    pub fn from_json_str(src: &str) -> Result<Self, String> {
        // Windows 上用 PowerShell / 记事本保存的 UTF-8 文件常带 BOM，serde_json 会直接
        // 报 "expected value at line 1 column 1"，这里统一剥掉以免用户以为场景写错了。
        let src = src.strip_prefix('\u{feff}').unwrap_or(src);
        let file: SceneFile =
            serde_json::from_str(src).map_err(|e| format!("场景 JSON 解析失败: {e}"))?;
        Self::from_file(file)
    }

    pub fn from_file(file: SceneFile) -> Result<Self, String> {
        let mut scene = Scene::new();
        scene.name = file.name.unwrap_or_else(|| "untitled".to_string());
        scene.physics = file.physics.unwrap_or_default();
        scene.assets = file.assets;

        if let Some(bg) = file.background {
            scene.background = Vec4::from(bg);
        }
        if let Some(cam) = file.camera {
            if let Some(p) = cam.position {
                scene.camera.position = Vec3::from(p);
            }
            if let Some(t) = cam.target {
                scene.camera.target = Vec3::from(t);
            }
            if let Some(f) = cam.fov {
                scene.camera.fov_deg = f;
            }
        }
        if let Some(l) = file.light {
            if let Some(d) = l.direction {
                let d = Vec3::from(d);
                if d.length_squared() > 1e-8 {
                    scene.light.direction = d.normalize();
                }
            }
            if let Some(c) = l.color {
                scene.light.color = Vec3::from(c);
            }
            if let Some(i) = l.intensity {
                scene.light.intensity = i;
            }
            if let Some(a) = l.ambient {
                scene.light.ambient = Vec3::from(a);
            }
            if let Some(sd) = l.shadow {
                let mut s = scene.light.shadow;
                if let Some(v) = sd.enabled {
                    s.enabled = v;
                }
                if let Some(v) = sd.size {
                    s.size = v;
                }
                if let Some(v) = sd.ortho {
                    s.ortho_size = v;
                }
                if let Some(v) = sd.bias {
                    s.bias = v;
                }
                if let Some(v) = sd.normal_bias {
                    s.normal_bias = v;
                }
                if let Some(v) = sd.distance {
                    s.distance = v;
                }
                if let Some(v) = sd.strength {
                    s.strength = v;
                }
                if let Some(v) = sd.cascades {
                    s.cascade.count = v;
                }
                if let Some(v) = sd.blend {
                    s.cascade.blend = v;
                }
                if let Some(v) = sd.cascade_distance {
                    s.cascade.max_distance = v;
                }
                if let Some(v) = sd.caster_extend {
                    s.cascade.caster_extend = v;
                }
                if let Some(v) = sd.caster_cull {
                    s.cascade.caster_cull = v;
                }
                scene.light.shadow = s.sanitized();
            }
        }

        // 环境探针整段覆盖：这是「一个完整的光源配置」，逐字段可缺省会得到四不像的光，
        // 缺省值统一由 EnvironmentSettings::default() 提供（未写的字段在反序列化时就已补默认）
        scene.environment = file.environment.map(|e| e.sanitized());
        // 后处理同样整段覆盖：缺段 = None（直通），有段则按 PostFxSettings 收敛
        scene.postfx = file.postfx.map(|p| p.sanitized());
        // 遮挡剔除：缺段 = 缺省（关闭），有段则收敛到合法区间
        scene.occlusion = file.occlusion.unwrap_or_default().sanitized();
        // HDR 反射探针：缺段 = None（沿用解析 IBL），有段则收敛路径与旋钮
        scene.environment_map = file.environment_map.map(|m| m.sanitized());
        // 局部光源：逐盏收敛 + 按 key 去重（后者覆盖前者），超上限的截断。
        // 空 key 是无效光源（脚本 / RPC 拿不到它），直接跳过而不是静默塞个默认名。
        for light in file.lights {
            if light.key.trim().is_empty() {
                continue;
            }
            let _ = light::upsert_local(&mut scene.lights, light);
        }

        // 场景材质表先落地：物件 spawn 时会带着 materialRef 进来，表在这里收敛一次
        scene.materials = scene_material::normalize_all(file.materials);
        for obj in file.objects {
            scene.spawn(obj)?;
        }
        // 地形：丢空名、同名后者覆盖前者、超上限截断（收敛在 core，加载与命令共用一套）
        scene.terrain = terrain::normalize_all(file.terrain);
        scene.spawns = spawn::normalize_all(file.spawns);
        scene.texts = file.texts.into_iter().map(|t| t.sanitized()).collect();
        scene.ui = file.ui.into_iter().map(|u| u.sanitized()).collect();
        scene.update_world_transforms();
        Ok(scene)
    }

    // ---------- 保存 ----------

    /// 把当前场景的**源数据**导回 [`SceneFile`]，与 [`Scene::from_file`] 严格对称。
    ///
    /// 只写「场景文件里本来就有的东西」：场景名、背景、相机、光照、场景级物理声明、
    /// 物件描述与资产声明。运行时派生状态一律不写：
    ///
    /// - 每帧由 [`Scene::update_world_transforms`] 算出的 `world` / `world_normal` 矩阵缓存；
    /// - 物理后端的刚体句柄 `body`（句柄只对本次运行的 `Physics` 有效）；
    /// - `next_id` / `children` / `time` / `frame` 这类引擎内部记账；
    /// - 动画实例、蒙皮实例、GPU 侧资源与实例索引——它们由应用层按资产声明在加载后重建。
    ///
    /// 于是 `load -> save -> load` 得到的场景与第一次加载语义等价。
    pub fn to_scene_file(&self) -> SceneFile {
        SceneFile {
            name: Some(self.name.clone()),
            background: Some(self.background.to_array()),
            camera: Some(CameraDesc {
                position: Some(self.camera.position.to_array()),
                target: Some(self.camera.target.to_array()),
                fov: Some(self.camera.fov_deg),
            }),
            light: Some(self.light_desc()),
            environment: self.environment.map(|e| e.sanitized()),
            postfx: self.postfx.map(|p| p.sanitized()),
            lights: self.lights.clone(),
            // 全是缺省值时不写这一段：旧场景 roundtrip 不该凭空长出 occlusion 字段
            occlusion: if self.occlusion == OcclusionSettings::default() {
                None
            } else {
                Some(self.occlusion.sanitized())
            },
            // 同理：没配探针时不该凭空长出 environment_map 段
            environment_map: self
                .environment_map
                .as_ref()
                .filter(|m| !m.is_default())
                .map(|m| m.clone().sanitized()),
            objects: self
                .order
                .iter()
                .filter_map(|id| self.nodes.get(id))
                .map(|node| self.node_desc(node))
                .collect(),
            terrain: self.terrain.clone(),
            spawns: self.spawns.clone(),
            materials: self.materials.clone(),
            physics: if physics_desc_is_empty(&self.physics) {
                None
            } else {
                Some(self.physics.clone())
            },
            assets: self.assets.clone(),
            texts: self.texts.clone(),
            ui: self.ui.clone(),
        }
    }

    /// 场景 → 紧凑 JSON 文本（`Scene::from_json_str` 的逆操作）。
    pub fn to_json_string(&self) -> Result<String, String> {
        serde_json::to_string(&self.to_scene_file()).map_err(|e| format!("场景序列化失败: {e}"))
    }

    /// 场景 → 带缩进、末尾带换行的 JSON 文本（给人读 / 进版本库的形态）。
    pub fn to_json_pretty(&self) -> Result<String, String> {
        let mut text = serde_json::to_string_pretty(&self.to_scene_file())
            .map_err(|e| format!("场景序列化失败: {e}"))?;
        text.push('\n');
        Ok(text)
    }

    /// 把场景写回文件；父目录不存在时自动创建，写入是整体覆盖。
    ///
    /// 返回的错误是给人看的中文串，与应用层其它读场景处的风格一致。
    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("创建场景目录 {} 失败: {e}", dir.display()))?;
            }
        }
        let text = self.to_json_pretty()?;
        std::fs::write(path, text).map_err(|e| format!("写入场景 {} 失败: {e}", path.display()))
    }

    /// 光照 → `light` 段。方向光方向统一写成单位向量（加载侧也会归一化）。
    fn light_desc(&self) -> LightDesc {
        let direction = self.light.direction;
        let direction = if direction.length_squared() > 1e-12 {
            direction.normalize()
        } else {
            Vec3::NEG_Y
        };
        let shadow = self.light.shadow;
        LightDesc {
            direction: Some(direction.to_array()),
            color: Some(self.light.color.to_array()),
            intensity: Some(self.light.intensity),
            ambient: Some(self.light.ambient.to_array()),
            shadow: Some(ShadowDesc {
                enabled: Some(shadow.enabled),
                size: Some(shadow.size),
                // 0 是「按包围球自适应」的哨兵值，写 0 与省略等价，省掉噪声
                ortho: (shadow.ortho_size > 0.0).then_some(shadow.ortho_size),
                bias: Some(shadow.bias),
                normal_bias: Some(shadow.normal_bias),
                distance: Some(shadow.distance),
                strength: Some(shadow.strength),
                // 1 级 + 缺省混合比是「没开 CSM」的等价形态，省掉噪声；
                // 一旦开了多级或调过混合比就必须写进文件，否则重开场景会静默退回单级。
                cascades: (shadow.cascade.count > 1).then_some(shadow.cascade.count),
                blend: (shadow.cascade.blend != light::CascadeSettings::default().blend)
                    .then_some(shadow.cascade.blend),
                cascade_distance: (shadow.cascade.max_distance > 0.0)
                    .then_some(shadow.cascade.max_distance),
                // 0 是「不扩张」的缺省形态，省略；`caster_cull` 缺省为 true，
                // 只有显式关掉时才写进文件（否则重开场景又会静默打开）。
                caster_extend: (shadow.cascade.caster_extend > 0.0)
                    .then_some(shadow.cascade.caster_extend),
                caster_cull: (!shadow.cascade.caster_cull).then_some(false),
            }),
        }
    }

    /// 单个物件 → `objects[]` 项。
    ///
    /// 旋转按欧拉角写回（场景文件的可读形态），退回时 `set_euler_deg` 还原成同一旋转；
    /// 身份旋转（欧拉角恰为 0）与 `visible = true` 这类缺省值省略，保持文件干净。
    fn node_desc(&self, node: &Node) -> ObjectDesc {
        let transform = &node.transform;
        let euler = transform.euler_deg();
        ObjectDesc {
            name: node.name.clone(),
            mesh: Some(node.mesh.as_string()),
            position: Some(transform.position.to_array()),
            rotation: (euler.length_squared() > 0.0).then_some(euler.to_array()),
            scale: Some(transform.scale.to_array()),
            color: Some(node.color.to_array()),
            parent: node
                .parent
                .and_then(|id| self.nodes.get(&id))
                .map(|parent| parent.name.clone()),
            visible: (!node.visible).then_some(false),
            physics: node.physics.clone(),
            material: node.material,
            material_ref: node.material_ref.clone(),
            // 空标签 / 不可交互不落盘：旧场景 roundtrip 不该凭空多出这两个字段
            tags: (!node.tags.is_empty()).then(|| node.tags.clone()),
            interact_radius: node.interact_radius,
        }
    }
}

/// 场景级物理配置是否「什么都没声明」：全 None 时整段省略，JSON 更干净。
fn physics_desc_is_empty(desc: &PhysicsSceneDesc) -> bool {
    desc.enabled.is_none()
        && desc.backend.is_none()
        && desc.gravity.is_none()
        && desc.ground.is_none()
        && desc.ccd.is_none()
}
