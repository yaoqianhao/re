use r3e_core::mesh::{MeshData, Vertex};
use r3e_core::MeshRef;

use crate::instance_upload::{self, MAX_RANGES, MERGE_GAP};
use crate::mesh::InstanceRaw;

use super::*;

impl Renderer {
    /// 扩展三十六：把形变表推到 GPU（仅当缓存版本与已上传版本不一致）。
    ///
    /// 不做脏段：表只有「新增网格」时才变，而新增一次就是几百 KiB 的整段写 ——
    /// 比维护影子副本 + 逐段比对更省。版本号相等时本函数退化为一次整数比较，
    /// 静态场景下每帧零开销、零上传。
    pub(crate) fn upload_morph_table(&mut self) {
        let version = self.mesh_cache.morph_version();
        if version == self.morph_uploaded_version {
            return;
        }
        // 借用分两步：先把内容搬进自有中转，再拿中转去写缓冲
        self.morph_scratch.clear();
        self.morph_scratch
            .extend_from_slice(self.mesh_cache.morph_table());
        self.morph_floats = self.morph_scratch.len() as u64;
        if !self.morph_scratch.is_empty() {
            self.queue.write_buffer(
                &self.morph_buffer,
                0,
                bytemuck::cast_slice(&self.morph_scratch),
            );
        }
        self.morph_uploaded_version = version;
        self.morph_uploads += 1;
        log::debug!(
            "形变表已上传（版本 {version}）：{} 个 float",
            self.morph_scratch.len()
        );
    }

    /// 扩展三十一：把本帧实例记录按脏段传上 GPU，并刷新影子副本。
    ///
    /// 与全量上传的唯一差别是「跳过了逐字节相同的记录」：`plan_upload` 只在
    /// 上一帧存在同长度影子时才给脏段，长度变化 / 换代 / 段数超限一律退化为
    /// 单段全量，因此正确性不依赖调用方做任何假设。
    ///
    /// 影子必须在**写完 GPU 之后**才更新 —— 上传中途 panic 时宁可下一帧多传一遍，
    /// 也不能让影子记着「传过了」而缓冲里其实还是旧数据。
    pub(crate) fn upload_instance_runs(&mut self, all: &[InstanceRaw]) {
        let stride = std::mem::size_of::<InstanceRaw>();
        let prev_bytes = bytemuck::cast_slice::<InstanceRaw, u8>(&self.instance_shadow);
        let next_bytes = bytemuck::cast_slice::<InstanceRaw, u8>(all);
        let plan =
            instance_upload::plan_upload(prev_bytes, next_bytes, stride, MERGE_GAP, MAX_RANGES);

        // 扩展三十五：优先把脏段打进暂存环 —— 一帧只做一次 `write_buffer`，
        // 逐段搬运改由命令编码器里的块拷贝完成（省的是调用开销，不是带宽）。
        // 装不下 / 超上限 / 开关关闭时 `stage` 返回 false，走下面的逐段直传。
        let staged = self.staging.stage(
            &self.device,
            StagingTarget::Instances,
            next_bytes,
            stride,
            &plan.ranges,
        );
        if staged {
            self.staging.set_instance_shadow(all);
        } else {
            for run in &plan.ranges {
                let start = run.start as usize * stride;
                let end = run.end as usize * stride;
                self.queue.write_buffer(
                    &self.instance_buffer,
                    run.start as u64 * stride as u64,
                    &next_bytes[start..end],
                );
            }
            // 直传路径当场生效，影子也当场刷新（与扩展三十一的不变量一致）
            self.instance_shadow.clear();
            self.instance_shadow.extend_from_slice(all);
        }

        self.instance_upload = InstanceUploadStats {
            ranges: plan.range_count(),
            recorded: plan.recorded,
            skipped: plan.skipped,
            bytes: plan.bytes,
            full_bytes: all.len() as u64 * stride as u64,
            full: plan.full,
            resets: self.instance_upload_resets,
        };
    }

    /// 扩展三十二：把本帧关节矩阵按脏段传上 GPU，并刷新影子副本。
    ///
    /// 与实例路径共用同一套 `plan_upload`，只把「记录」从 `InstanceRaw`（112 B）
    /// 换成 `mat4`（64 B）：空洞合并（`MERGE_GAP`）与段数上限（`MAX_RANGES`）
    /// 沿用同一个启发式，因为二者对 `write_buffer` 调用次数同样敏感。
    ///
    /// 关节矩阵按**入桶顺序**排进缓冲，所以实例顺序一变（半透明重排、剔除
    /// 结果变化）槽位就跟着挪位，脏段会变多 —— 那是保守的多传，不影响正确性。
    /// 蒙皮实例数变化时长度不等，`plan_upload` 直接退化为全量；关节缓冲容量
    /// 固定、从不换代，因此这里不需要处理「影子失效」以外的退化路径。
    pub(crate) fn upload_joint_runs(&mut self, joints: &[[f32; 16]]) {
        if joints.is_empty() {
            // 本帧没有蒙皮实例：影子清空，下一帧若又有蒙皮会自动走全量
            self.joint_shadow.clear();
            self.joint_upload = InstanceUploadStats::default();
            return;
        }
        let stride = std::mem::size_of::<[f32; 16]>();
        let prev_bytes = bytemuck::cast_slice::<[f32; 16], u8>(&self.joint_shadow);
        let next_bytes = bytemuck::cast_slice::<[f32; 16], u8>(joints);
        let plan =
            instance_upload::plan_upload(prev_bytes, next_bytes, stride, MERGE_GAP, MAX_RANGES);

        // 扩展三十五：与实例路径共用同一个暂存槽（同一帧一次写入），
        // 记录是 mat4（64 B），拷贝目标换成关节缓冲。
        let staged = self.staging.stage(
            &self.device,
            StagingTarget::Joints,
            next_bytes,
            stride,
            &plan.ranges,
        );
        if staged {
            self.staging.set_joint_shadow(joints);
        } else {
            for run in &plan.ranges {
                let start = run.start as usize * stride;
                let end = run.end as usize * stride;
                self.queue.write_buffer(
                    &self.joint_buffer,
                    run.start as u64 * stride as u64,
                    &next_bytes[start..end],
                );
            }
            self.joint_shadow.clear();
            self.joint_shadow.extend_from_slice(joints);
        }

        self.joint_upload = InstanceUploadStats {
            ranges: plan.range_count(),
            recorded: plan.recorded,
            skipped: plan.skipped,
            bytes: plan.bytes,
            full_bytes: joints.len() as u64 * stride as u64,
            full: plan.full,
            resets: 0,
        };
    }

    /// 取网格材质自身的 alpha（无材质 / 无贴图时按 1.0 处理）。
    ///
    /// 半透明判定要同时看两处 alpha：节点颜色带一个，网格 PBR 材质还能带一个。
    /// 只看节点颜色会让「材质设成半透明、颜色保持不透明」的物件排错批次。
    pub(crate) fn mesh_material_alpha(&self, mesh: MeshRef) -> f32 {
        self.mesh_cache
            .material_alpha(mesh)
            .map_or(1.0, |alpha| alpha.clamp(0.0, 1.0))
    }
}

/// 上传一份 CPU 网格到 GPU。`parts` 为空时按「整网格一段默认材质」补齐。
pub(crate) fn upload_mesh(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data: &MeshData,
    mut parts: Vec<GpuPart>,
) -> GpuMesh {
    let index_count = data.indices.len() as u32;
    if parts.is_empty() && index_count > 0 {
        parts.push(GpuPart {
            first_index: 0,
            index_count,
            material: MaterialKey::Default,
        });
    } else {
        // 资产层给的分区若越界（数据错误），裁到索引范围内，避免 draw 越界。
        for part in &mut parts {
            if part.first_index >= index_count {
                part.index_count = 0;
                continue;
            }
            part.index_count = part
                .index_count
                .min(index_count.saturating_sub(part.first_index));
        }
        parts.retain(|p| p.index_count > 0);
        if parts.is_empty() && index_count > 0 {
            parts.push(GpuPart {
                first_index: 0,
                index_count,
                material: MaterialKey::Default,
            });
        }
    }
    let vertex_size = (data.vertices.len().max(1) * std::mem::size_of::<Vertex>()) as u64;
    let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("r3e-vertex-buffer"),
        size: vertex_size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    if !data.vertices.is_empty() {
        queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(&data.vertices));
    }

    let index_size = (data.indices.len().max(1) * std::mem::size_of::<u32>()) as u64;
    let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("r3e-index-buffer"),
        size: index_size,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    if !data.indices.is_empty() {
        queue.write_buffer(&index_buffer, 0, bytemuck::cast_slice(&data.indices));
    }

    GpuMesh {
        vertex_buffer,
        index_buffer,
        parts,
        // 形变登记要往全局表里追加，只能由 `MeshCache::ensure` 在拿到 `&mut self`
        // 之后填，`upload_mesh` 这个自由函数不认识缓存。
        morph: None,
    }
}
