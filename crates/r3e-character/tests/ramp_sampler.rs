//! B04 辅助判据：采样器口径与轨迹确定性。
//!
//! `ramp_accuracy.rs` 负责「误差 < 2cm」，本文件负责两件更基础的事：
//!
//! 1. **采样器自身可信**：帧数不足时不能凭空产出采样点，且每点记的必须是**当帧**脚底
//!    高度（不是采样时刻重算一次）。采样器错了，2cm 判据就只是自说自话。
//! 2. **轨迹可复现**：判据依赖「同一配置 + 同一输入 → 同一轨迹」。两后端各跑一遍同一段
//!    输入，逐位比较脚底坐标——若轨迹本身漂，2cm 的容差就成了掩盖不确定性的橡皮尺。
//!
//! 逐位比较（`to_bits`）而不是按容差近似：这里要证明的是**没有不确定性**，
//! 用容差比就把要证明的东西假设掉了。

mod common;

use common::*;
use glam::Vec3;
use r3e_character::{
    CharacterConfig, CharacterSolver, CharacterState, KinematicSolver, MoveIntent, FIXED_DT,
};
use r3e_physics::port::PhysicsQueryPort;
use r3e_physics::{Physics, PhysicsKind};

/// 采样间隔（帧），与 `ramp_accuracy.rs` 同口径。
const SAMPLE_EVERY: usize = 10;

/// 判据坡度：取最陡的一档，数值路径最容易分叉。
const DEG: f32 = 40.0;

/// 踏板半长（米）。
const RAMP_HALF_LEN: f32 = 12.0;

/// 一次采样记录。
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sample {
    frame: usize,
    feet_y: f32,
}

/// 采样器：每 `every` 帧记一次调用方给出的当帧高度。
///
/// 抽成一个结构而不是就地 `if frame % every == 0`，是为了让「采样器本身」可被断言：
/// 帧数不够时它必须返回空，而不是补一个第 0 帧的假样本。
struct Sampler {
    every: usize,
    frames: usize,
    out: Vec<Sample>,
}

impl Sampler {
    fn new(every: usize) -> Self {
        assert!(every > 0, "采样间隔必须为正");
        Self {
            every,
            frames: 0,
            out: Vec::new(),
        }
    }

    /// 推进一帧，`feet_y` 是**该帧结束时刻**的脚底高度。
    fn tick(&mut self, feet_y: f32) {
        self.frames += 1;
        if self.frames % self.every == 0 {
            self.out.push(Sample {
                frame: self.frames,
                feet_y,
            });
        }
    }

    fn samples(&self) -> &[Sample] {
        &self.out
    }
}

fn ramp_port(kind: PhysicsKind, deg: f32) -> Box<dyn PhysicsQueryPort> {
    match kind {
        PhysicsKind::Builtin => Box::new(World::slope(0.0, deg)),
        _ => {
            let physics: Physics = with_ramp(floor_world(kind), deg, RAMP_HALF_LEN);
            Box::new(physics)
        }
    }
}

fn solver() -> KinematicSolver {
    KinematicSolver
}

fn config() -> CharacterConfig {
    CharacterConfig::default()
}

/// 站稳后直行 `frames` 帧，返回逐帧脚底轨迹（**不采样**，用于逐位比较）。
fn walk_track(kind: PhysicsKind, frames: usize) -> Vec<[u32; 3]> {
    let port = ramp_port(kind, DEG);
    let mut state = CharacterState::new(Vec3::new(-1.0, 0.5, 0.0));
    let intent = MoveIntent {
        wish_dir: Vec3::ZERO,
        jump: false,
    };
    for _ in 0..60 {
        solver().step(&config(), &mut state, intent, FIXED_DT, port.as_ref());
    }
    assert!(state.grounded, "{kind:?}：起步前没站稳");

    let mut track = Vec::with_capacity(frames);
    let forward = MoveIntent {
        wish_dir: Vec3::X,
        jump: false,
    };
    for _ in 0..frames {
        solver().step(&config(), &mut state, forward, FIXED_DT, port.as_ref());
        let f = state.feet;
        track.push([f.x.to_bits(), f.y.to_bits(), f.z.to_bits()]);
    }
    track
}

#[test]
fn sampler_yields_nothing_without_enough_frames() {
    let mut s = Sampler::new(SAMPLE_EVERY);
    for _ in 0..(SAMPLE_EVERY - 1) {
        s.tick(0.5);
    }
    assert!(
        s.samples().is_empty(),
        "不足一个采样间隔就产样本，等于把第 0 帧当前置真值"
    );

    s.tick(0.75);
    assert_eq!(s.samples().len(), 1);
    assert_eq!(s.samples()[0].frame, SAMPLE_EVERY);
    assert_eq!(s.samples()[0].feet_y, 0.75, "记的必须是当帧高度");
}

#[test]
fn sampler_keeps_per_frame_value_and_error_is_measurable() {
    // 线性抬升：第 i 帧高度 = i * 0.01，期望坡面高度同式 → 差值必须逐点为 0；
    // 再人为给一个 3cm 的偏移，采样器必须把 0.03 原样暴露出来（不吞不夹）。
    let mut clean = Sampler::new(SAMPLE_EVERY);
    let mut biased = Sampler::new(SAMPLE_EVERY);
    for i in 1..=30 {
        let truth = i as f32 * 0.01;
        clean.tick(truth);
        biased.tick(truth + 0.03);
    }

    let clean: Vec<f32> = clean
        .samples()
        .iter()
        .map(|s| s.feet_y - s.frame as f32 * 0.01)
        .collect();
    let biased: Vec<f32> = biased
        .samples()
        .iter()
        .map(|s| s.feet_y - s.frame as f32 * 0.01)
        .collect();
    assert_eq!(clean.len(), 3);
    assert!(
        clean.iter().all(|d| *d == 0.0),
        "无偏差时差值必须为 0：{clean:?}"
    );
    assert!(
        biased.iter().all(|d| (*d - 0.03).abs() < 1e-6),
        "3cm 偏差必须原样出现：{biased:?}"
    );
}

#[test]
fn track_is_bitwise_identical_on_rerun_rapier() {
    if !PhysicsKind::Rapier.is_available() {
        eprintln!("跳过：Rapier 未编译进本构建");
        return;
    }
    let a = walk_track(PhysicsKind::Rapier, 60);
    let b = walk_track(PhysicsKind::Rapier, 60);
    assert_eq!(a, b, "同配置同输入两次跑出不同轨迹，2cm 判据失去意义");
}

#[test]
fn track_is_bitwise_identical_on_rerun_analytic() {
    let a = walk_track(PhysicsKind::Builtin, 60);
    let b = walk_track(PhysicsKind::Builtin, 60);
    assert_eq!(a, b, "解析世界两次跑出不同轨迹，判据不可复现");
}
