//! 确定性回放器（M0-S4-05）。
//!
//! 目标只有一个：**同样的种子跑两遍，逐帧哈希必须一模一样**。为此这里只依赖真值
//! （`r3e-core`）与文档层（`r3e-doc`），不建窗口、不开音频、不碰物理——没有设备抖动，
//! CI 上不会因为无声卡而红。
//!
//! # 为什么复用文档层而不是直接改 `Scene`
//!
//! 回放要覆盖的正是「脚本/编辑器的写入路径」：节拍推进走 `Document::write`，动作走
//! 命令层（`CommandKind`），成批走 `Transaction`。绕过它们去直接改 `Scene` 只能验证
//! 哈希函数，验证不了 S3/S4 真正上线的那条链路。
//!
//! # 每帧做什么
//!
//! 1. 节拍：`doc.write(|s| { s.time += DT; s.frame += 1 })`；
//! 2. 动作：前 [`WARMUP_FRAMES`] 帧每帧 `SpawnNode`（名字 `n{seq:03}`，作为后续动作的
//!    作用对象）；之后每帧在**已到场**的节点里挑一个，随机命中
//!    `SetTransform` / `SetVisible` / `SetNodeColor`；
//! 3. 攒批：命令先排队，每 [`TRANSACTION_EVERY`] 帧一次
//!    `begin → n×stage → commit`，把这一批打成**一笔**事务——事务路径本身也进覆盖；
//! 4. 采样：每 `hash_every` 帧记一条 [`ReplaySample`]。
//!
//! # 命令前像怎么保证真实
//!
//! 批内命令在**影子场景**上按序预演后构造：第 2 条命令抓的前像是第 1 条执行后的值，
//! 而不是批开始前的值。这样整批 `undo` 逆序写回时每一步都精确，任意时刻回退都不会
//! 出现「中间态串味」。
//!
//! # 随机数
//!
//! 手写 64 位 LCG（PCG 常数），不引 `rand`：跨机器、跨编译器版本必须逐位一致，
//! 引入外部 crate 等于把「确定性」押在别人的实现细节上。

use r3e_core::{ObjectDesc, Scene, Transform, Vec3, Vec4};
use r3e_doc::{Command, CommandKind, DocError, Document, StateHash, Transaction};

/// 固定步长：与 `r3e_runtime::HEADLESS_FIXED_DT` 同值（1/60 秒）。
///
/// 刻意不从 runtime 引用（那会把 render/audio/physics 全拖进来），两处各自定义常量、
/// 各自在文档里互相引用。
pub const REPLAY_DT: f32 = 1.0 / 60.0;

/// 预热帧数：这一段时间里每帧新建一个节点，给后续动作备好作用对象。
pub const WARMUP_FRAMES: u64 = 60;

/// 多少帧打成一笔事务。
pub const TRANSACTION_EVERY: u64 = 10;

/// 回放参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayConfig {
    /// 随机种子。
    pub seed: u64,
    /// 总帧数。
    pub frames: u64,
    /// 采样间隔（帧）。`0` 视为不采样。
    pub hash_every: u64,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            seed: 42,
            frames: 600,
            hash_every: 60,
        }
    }
}

impl ReplayConfig {
    /// 打一次参数横幅（`main` 与测试共用，保证两处格式一致）。
    pub fn banner(&self) -> String {
        format!(
            "[replay] seed={} frames={} hash_every={} dt={:.6}",
            self.seed, self.frames, self.hash_every, REPLAY_DT
        )
    }
}

/// 一个采样点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplaySample {
    /// 帧号（从 1 起）。
    pub frame: u64,
    /// 世界指纹：内容 + 节拍。
    pub world: StateHash,
    /// 内容指纹：不含节拍。
    pub state: StateHash,
    /// 场景节点数（可读量，用来定位分叉来源）。
    pub nodes: usize,
    /// 撤销栈笔数。
    pub depth: usize,
}

impl ReplaySample {
    /// 一行的行体（不含换行），`main` 与测试共用。
    pub fn line(&self) -> String {
        format!(
            "frame {:04} world={} state={} nodes={} depth={}",
            self.frame, self.world, self.state, self.nodes, self.depth
        )
    }
}

/// 回放报告。
#[derive(Debug, Clone)]
pub struct ReplayReport {
    /// 本次参数。
    pub config: ReplayConfig,
    /// 采样点（按帧升序）。
    pub samples: Vec<ReplaySample>,
    /// 提交的事务笔数。
    pub commits: u64,
    /// 提交的命令条数（含预热期的 spawn）。
    pub commands: u64,
    /// 撤销栈最终笔数。
    pub undo_depth: usize,
    /// 最终世界指纹。
    pub final_world: StateHash,
    /// 最终内容指纹。
    pub final_state: StateHash,
}

impl ReplayReport {
    /// 收尾横幅（`main` 与测试共用）。
    pub fn footer(&self) -> String {
        format!(
            "[replay] done frames={} commits={} undo_depth={} final={}",
            self.config.frames, self.commits, self.undo_depth, self.final_world
        )
    }
}

/// 回放错误。
#[derive(Debug)]
pub enum ReplayError {
    /// 文档层拒绝（命令非法、事务回滚失败等）。
    Doc(DocError),
    /// 场景文件读不进来。
    Scene(String),
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Doc(e) => write!(f, "文档层错误：{e}"),
            Self::Scene(e) => write!(f, "场景错误：{e}"),
        }
    }
}

impl std::error::Error for ReplayError {}

impl From<DocError> for ReplayError {
    fn from(e: DocError) -> Self {
        Self::Doc(e)
    }
}

/// 64 位线性同余发生器（PCG 常数）。
///
/// 只做一件事：稳定地产出与机器无关的伪随机序列。
#[derive(Debug, Clone)]
pub struct Lcg(u64);

impl Lcg {
    /// 以种子建发生器（种子 `0` 也给出非零周期起点）。
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    /// 推进并返回状态。
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    /// `[0, n)` 内的取值（`n == 0` 时返回 0）。
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }

    /// 在已到场的节点里挑一个名字（命名规则见 [`node_name`]）。
    fn pick_node(&mut self, spawned: u64) -> String {
        node_name(self.below(spawned.max(1)))
    }
}

/// 预热期第 `seq` 个节点的名字（`n000` / `n001` …）。
pub fn node_name(seq: u64) -> String {
    format!("n{seq:03}")
}

/// 按配置跑一遍回放（自建空文档）。
pub fn run(config: &ReplayConfig) -> Result<ReplayReport, ReplayError> {
    run_on(config, Document::new(Scene::new()))
}

/// 在给定文档上跑一遍回放：命令全部落在传进来的文档上，调用方可继续检查。
///
/// 文档会被推进 `frames` 帧；调用前的内容作为回放起点。
pub fn run_on(config: &ReplayConfig, mut doc: Document) -> Result<ReplayReport, ReplayError> {
    let mut rng = Lcg::new(config.seed);
    let mut samples = Vec::new();
    let mut commits = 0u64;
    let mut commands = 0u64;
    let mut spawned = 0u64;

    let mut frame = 0u64;
    while frame < config.frames {
        // 一批 = 最多 TRANSACTION_EVERY 帧，批内命令排队后打成一笔。
        let batch_len = TRANSACTION_EVERY.min(config.frames - frame);
        let mut batch: Vec<CommandKind> = Vec::with_capacity(batch_len as usize);
        // 影子场景：批内命令在这里按序预演，保证每条命令的前像都是「执行到它时的值」。
        let mut shadow = doc.as_scene().clone();

        for _ in 0..batch_len {
            frame += 1;
            // ① 世界节拍：走文档唯一写入口。
            doc.write(|s| {
                s.time += REPLAY_DT;
                s.frame += 1;
            });
            shadow.time = doc.as_scene().time;
            shadow.frame = doc.as_scene().frame;

            // ② 派生动作。
            let cmd = if frame <= WARMUP_FRAMES {
                let desc = ObjectDesc {
                    name: node_name(spawned),
                    ..ObjectDesc::default()
                };
                spawned += 1;
                CommandKind::SpawnNode { desc }
            } else {
                pick_action(&shadow, &mut rng, spawned)
            };
            // 影子预演：让下一条命令看到的是「它真正执行时」的场景。
            Command::apply(&cmd, &mut shadow)?;
            batch.push(cmd);

            // ③ 攒满批就提交（整笔一次进撤销栈）。
            if batch.len() as u64 == batch_len {
                let mut tx: Transaction<'_> = doc.begin();
                for cmd in batch.drain(..) {
                    tx.stage(move |_| Ok(cmd))?;
                }
                tx.commit()?;
                commits += 1;
                commands += batch_len;
            }

            // ④ 采样。
            if config.hash_every > 0 && frame % config.hash_every == 0 {
                samples.push(ReplaySample {
                    frame,
                    world: doc.world_hash(),
                    state: doc.state_hash(),
                    nodes: doc.as_scene().len(),
                    depth: doc.undo_depth(),
                });
            }
        }
    }

    Ok(ReplayReport {
        config: config.clone(),
        samples,
        commits,
        commands,
        undo_depth: doc.undo_depth(),
        final_world: doc.world_hash(),
        final_state: doc.state_hash(),
    })
}

/// 从影子场景挑一个节点，随机命中三种动作之一。
///
/// 三种动作都只改「命令层覆盖到的字段」，因此回放既能抓到哈希分叉，也能抓到
/// 命令 `undo` 的错位。
fn pick_action(shadow: &Scene, rng: &mut Lcg, spawned: u64) -> CommandKind {
    let name = rng.pick_node(spawned);
    // 名字一定存在（预热期已建），兜底给一个空变换命令而不是 panic。
    let Some(id) = shadow.find_id(&name) else {
        return CommandKind::SetTransform {
            id: 0,
            old: Transform::default(),
            new: Transform::default(),
        };
    };
    let node = shadow.get(id).expect("刚查到的节点必然存在");
    match rng.below(3) {
        0 => CommandKind::SetTransform {
            id,
            old: node.transform,
            new: Transform::from_position(Vec3::new(
                rng.below(64) as f32 * 0.5,
                rng.below(64) as f32 * 0.25,
                rng.below(64) as f32 * 0.125,
            )),
        },
        1 => CommandKind::SetVisible {
            id,
            old: node.visible,
            new: !node.visible,
        },
        _ => CommandKind::SetNodeColor {
            id,
            old: node.color,
            new: Vec4::new(
                rng.below(256) as f32 / 255.0,
                rng.below(256) as f32 / 255.0,
                rng.below(256) as f32 / 255.0,
                1.0,
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(frames: u64) -> ReplayConfig {
        ReplayConfig {
            seed: 7,
            frames,
            hash_every: 10,
        }
    }

    #[test]
    fn same_seed_same_hashes() {
        let a = run(&cfg(120)).expect("回放 A");
        let b = run(&cfg(120)).expect("回放 B");
        assert_eq!(a.samples, b.samples, "同种子两次回放必须逐采样点一致");
        assert_eq!(a.final_world, b.final_world);
        assert_eq!(a.final_state, b.final_state);
    }

    #[test]
    fn different_seed_diverges() {
        let a = run(&ReplayConfig {
            seed: 1,
            frames: 120,
            hash_every: 10,
        })
        .expect("回放 A");
        let b = run(&ReplayConfig {
            seed: 2,
            frames: 120,
            hash_every: 10,
        })
        .expect("回放 B");
        assert_ne!(a.final_state, b.final_state, "换种子必须换出不同状态");
    }

    #[test]
    fn transactions_are_one_undo_step() {
        let report = run(&cfg(120)).expect("回放");
        assert_eq!(report.commits, 12, "120 帧 / 每 10 帧一笔");
        assert_eq!(report.undo_depth, 12, "每笔占撤销栈一格");
        assert_eq!(report.commands, 120);
    }

    #[test]
    fn warmup_spawns_one_node_per_frame() {
        let report = run(&cfg(WARMUP_FRAMES)).expect("回放");
        assert_eq!(
            report.samples.last().expect("有采样").nodes as u64,
            WARMUP_FRAMES,
            "预热期每帧一个节点"
        );
    }

    #[test]
    fn world_hash_tracks_frame_progress() {
        let report = run(&cfg(30)).expect("回放");
        let mut prev = 0;
        for (i, s) in report.samples.iter().enumerate() {
            if i > 0 {
                assert_ne!(
                    s.world,
                    report.samples[i - 1].world,
                    "节拍推进必须改世界指纹"
                );
            }
            prev += 1;
        }
        assert_eq!(prev, report.samples.len());
    }

    #[test]
    fn hash_every_zero_means_no_samples() {
        let report = run(&ReplayConfig {
            seed: 3,
            frames: 20,
            hash_every: 0,
        })
        .expect("回放");
        assert!(report.samples.is_empty());
        assert_eq!(report.commits, 2);
    }

    #[test]
    fn node_names_are_padded() {
        assert_eq!(node_name(0), "n000");
        assert_eq!(node_name(7), "n007");
        assert_eq!(node_name(599), "n599");
    }

    #[test]
    fn lcg_is_stable_across_calls() {
        let mut a = Lcg::new(42);
        let mut b = Lcg::new(42);
        for _ in 0..16 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }
}
