use r3e_core::Scene;

use crate::mesh::InstanceRaw;

use super::*;

impl Renderer {
    /// 阶段 D：把阴影贴图尺寸对齐场景配置；返回本帧是否要跑阴影通道。
    /// 扩展十五：把场景里的局部光源打包上传，返回本帧生效的灯数。
    ///
    /// 每帧都传（而不是像球谐那样缓存）：灯可以被脚本逐帧移动，判等成本反而更高。
    /// 总开关关闭时计数写 0，GPU 侧整个循环跳过——数据仍留在场景里，打开即恢复。
    pub(crate) fn prepare_local_lights(&mut self, scene: &Scene) -> u32 {
        let uniform = LightsUniform::from_lights(&scene.lights, scene.lights_enabled);
        let count = uniform.count[0] as u32;
        self.queue
            .write_buffer(&self.lights_buffer, 0, bytemuck::bytes_of(&uniform));
        count
    }

    /// 把一批分组画进一个已经绑好相机 / 关节 / 阴影的通道。
    ///
    /// 返回 `(draw call 数, 实例数)`：`draw call` 按材质分区计数，
    /// 实例数按区间长度累加，两者都不含被剔掉的网格（`mesh_cache` 里没有的）。
    /// 不透明段与半透明段共用这段逻辑，区别只在调用前 `set_pipeline` 的是哪条管线。
    pub(crate) fn draw_group_range(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        groups: &[DrawGroup],
    ) -> (u32, u32) {
        let stride = std::mem::size_of::<InstanceRaw>() as u64;
        let mut draw_calls = 0u32;
        let mut instances = 0u32;
        for (key, range, surface, scene_material) in groups {
            let Some(gpu) = self.mesh_cache.get(*key) else {
                continue;
            };
            pass.set_vertex_buffer(0, gpu.vertex_buffer.slice(..));
            pass.set_vertex_buffer(
                1,
                self.instance_buffer
                    .slice(range.start as u64 * stride..range.end as u64 * stride),
            );
            pass.set_index_buffer(gpu.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

            // 每个材质分区一次 draw：bind group 1 换成该分区材质（含物件级 PBR 覆盖）。
            for part in &gpu.parts {
                // S4-A：场景材质整份取代网格资产材质与物件级覆盖
                let material_key = match scene_material {
                    Some(material) => MaterialKey::Scene(*material),
                    None => part.material.with_surface(*surface),
                };
                let Some(material) = self
                    .material_cache
                    .get(material_key)
                    .or_else(|| self.material_cache.get(MaterialKey::Default))
                else {
                    continue;
                };
                pass.set_bind_group(1, &material.bind_group, &[]);
                pass.draw_indexed(
                    part.first_index..part.first_index + part.index_count,
                    0,
                    0..1,
                );
                draw_calls += 1;
            }
            instances += range.end - range.start;
        }
        (draw_calls, instances)
    }

    /// 收齐本帧要用到的材质并建好 bind group（默认材质也要有一份）。
    pub(crate) fn prepare_materials(&mut self, groups: &[DrawGroup]) {
        let mut keys: Vec<MaterialKey> = vec![MaterialKey::Default];
        for (key, _, surface, scene_material) in groups {
            let Some(gpu) = self.mesh_cache.get(*key) else {
                continue;
            };
            for part in &gpu.parts {
                // 网格分区材质 + 物件级覆盖 → 本批次真正要绑的材质键。
                // S4-A：批次带场景材质时由它整份取代，网格资产材质不参与。
                let material = match scene_material {
                    Some(material) => MaterialKey::Scene(*material),
                    None => part.material.with_surface(*surface),
                };
                if !keys.contains(&material) {
                    keys.push(material);
                }
            }
        }
        let library = self.mesh_cache.provider().clone();
        for key in keys {
            self.material_cache
                .ensure(&self.device, &self.queue, Some(&library), key);
        }
    }
}
