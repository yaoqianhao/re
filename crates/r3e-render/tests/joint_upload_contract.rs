//! 扩展三十二：关节矩阵脏段上传的 **渲染层接入契约**（不开窗口、不建 GPU）。
//!
//! 关节缓冲与实例缓冲的差别决定了这一层要单独钉：
//!
//! * 记录换成 mat4（64 B），stride 一错就整体错位 —— 位移列会被写到缩放列上，
//!   表现为某个角色整体缩成一团，静态截图里极难发现；
//! * 关节槽位按**入桶顺序**分配，蒙皮实例数变化时长度不等必须退全量，
//!   否则上一帧的矩阵会被留在别的槽位上，角色骨骼错乱；
//! * 关节缓冲容量固定、从不换代，所以 `resets` 恒为 0 —— 这个字段若开始增长，
//!   说明有人给关节缓冲也加了扩容，影子同步逻辑必须跟着改；
//! * 无蒙皮帧必须清影子：留着旧影子会让下一帧的第一个蒙皮角色跳过首次上传，
//!   那一帧它会带着未初始化矩阵作画。

use std::mem::size_of;

use r3e_render::instance_upload::{plan_upload, MAX_RANGES, MERGE_GAP};
use r3e_render::renderer::{InstanceUploadStats, RenderStats};

/// 关节矩阵在 GPU 缓冲里的步长必须是 64 字节（mat4 = 16 × f32）。
#[test]
fn joint_record_stride_is_sixty_four_bytes() {
    assert_eq!(size_of::<[f32; 16]>(), 64);
    assert_eq!(
        size_of::<[f32; 16]>() % 4,
        0,
        "storage buffer 要求 4 字节对齐"
    );
}

/// 读数默认全零：没跑起来之前不能有任何「传过字节」的迹象。
#[test]
fn defaults_report_nothing_uploaded() {
    let stats = InstanceUploadStats::default();
    assert_eq!(stats.bytes, 0);
    assert_eq!(stats.skipped, 0);
    assert!(!stats.full);
    assert_eq!(stats.resets, 0);

    let stats = RenderStats::default();
    assert_eq!(stats.joint_upload.bytes, 0);
    assert_eq!(stats.joint_upload.recorded, 0);
    assert_eq!(stats.joint_upload.full_bytes, 0);
    assert_eq!(stats.joint_upload.resets, 0, "关节缓冲不换代");
}

/// 静止姿势：全部矩阵逐字节相同 → 一个字节都不传。
#[test]
fn static_pose_uploads_zero_bytes() {
    let stride = size_of::<[f32; 16]>();
    let joints = vec![5u8; stride * 512];
    let plan = plan_upload(&joints, &joints, stride, MERGE_GAP, MAX_RANGES);
    assert!(plan.is_empty());
    assert_eq!(plan.bytes, 0);
    assert_eq!(plan.recorded, 0);
    assert_eq!(plan.skipped, 512);
}

/// 上千个矩阵里只有一个角色的骨骼动了：脏段收缩到那个角色占的槽位段。
#[test]
fn one_animated_skeleton_uploads_its_own_slots() {
    let stride = size_of::<[f32; 16]>();
    let total = 1024usize;
    let prev = vec![0u8; stride * total];
    let mut next = prev.clone();
    // 第 700..732 号槽位（一个 32 骨骼角色）整体换了姿势
    for slot in 700..732usize {
        next[slot * stride + 12 * 4] = 1;
    }
    let plan = plan_upload(&prev, &next, stride, MERGE_GAP, MAX_RANGES);
    assert_eq!(plan.range_count(), 1);
    assert_eq!(plan.ranges[0], 700..732);
    assert_eq!(plan.bytes, 32 * stride as u64);
    assert_eq!(plan.skipped as usize, total - 32);
    assert!(!plan.full);
}

/// 蒙皮实例数变化（角色进场 / 退场）：长度不等只能整张重传。
#[test]
fn skinned_count_change_forces_full_upload() {
    let stride = size_of::<[f32; 16]>();
    let prev = vec![1u8; stride * 60];
    let next = vec![1u8; stride * 92];
    let plan = plan_upload(&prev, &next, stride, MERGE_GAP, MAX_RANGES);
    assert!(plan.full);
    assert_eq!(plan.ranges, vec![0..92]);
    assert_eq!(plan.bytes, (stride * 92) as u64);
}

/// 统计口径自洽：`recorded + skipped` = 矩阵总数，`bytes = recorded × 64`。
#[test]
fn stats_are_self_consistent() {
    let stride = size_of::<[f32; 16]>();
    let total = 256usize;
    let prev = vec![0u8; stride * total];
    let mut next = prev.clone();
    for slot in [0usize, 40, 41, 200] {
        next[slot * stride] = 3;
    }
    let plan = plan_upload(&prev, &next, stride, MERGE_GAP, MAX_RANGES);
    assert_eq!(plan.recorded + plan.skipped, total as u32);
    assert_eq!(plan.bytes, plan.recorded as u64 * stride as u64);
}

/// 接线：关节上传必须走脏段函数，旧的整段全量写必须已经删掉。
#[test]
fn renderer_uploads_joints_through_dirty_runs() {
    let src = include_str!("../src/renderer.rs");
    assert!(
        src.contains("self.upload_joint_runs(&joints)"),
        "prepare_instances 必须把关节上传交给 upload_joint_runs"
    );
    assert!(
        !src.contains("bytemuck::cast_slice(&self.joint_scratch)"),
        "旧的整张关节全量写必须删掉，否则脏段白算"
    );
    assert!(
        src.contains("joint_upload: self.joint_upload"),
        "RenderStats 组装必须带上关节读数，否则探针恒为 0"
    );
}

/// 接线：影子必须在写 GPU 之后刷新，空帧必须清影子。
#[test]
fn joint_shadow_follows_gpu_writes() {
    let src = include_str!("../src/renderer.rs");
    let start = src.find("fn upload_joint_runs").expect("上传函数必须存在");
    let body = &src[start..];
    let write_at = body.find("write_buffer(").expect("必须真的写 GPU");
    let shadow_at = body
        .find("self.joint_shadow.extend_from_slice(joints)")
        .expect("必须刷新影子");
    assert!(
        write_at < shadow_at,
        "影子必须在写 GPU 之后再刷新，否则中断一次就永久漏传"
    );

    let empty_at = body.find("if joints.is_empty()").expect("必须处理无蒙皮帧");
    let clear_at = body
        .find("self.joint_shadow.clear()")
        .expect("空帧要清影子");
    assert!(
        empty_at < clear_at,
        "空蒙皮帧必须清影子，否则下一个蒙皮角色首帧漏传"
    );
}
