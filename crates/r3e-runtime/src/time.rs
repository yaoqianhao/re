//! 帧时间统计：主循环采样，编辑器 / 诊断面板读取。
//!
//! 这两块状态属于「运行时时间步」，所以放在 runtime 而不是编辑器宿主：
//! 主循环负责 push，任何观察者（RPC、日志、headless 报告）只做 snapshot。

/// 帧时间窗口的容量：约 4 秒 @60fps。
///
/// 太短会让 p99 落在噪声上（一帧抖动就顶满），太长则改完场景要等好几秒才看得到
/// 分位变化。120 帧是两者之间的折中。
pub const FRAME_TIME_WINDOW: usize = 120;

/// 最近若干帧的 CPU 帧时间分布（毫秒），`engine.stats` 的 `frame.frame_ms` 就是它。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameTimeStats {
    /// 窗口里实际有多少个样本（启动初期远小于容量）。
    pub samples: usize,
    /// 最近一帧的耗时。
    pub last_ms: f32,
    pub p50_ms: f32,
    pub p95_ms: f32,
    pub p99_ms: f32,
    pub max_ms: f32,
}

impl FrameTimeStats {
    /// 从一批样本算出分布。
    ///
    /// 分位用 nearest-rank（`ceil(p·n)` 后取该名次），不是插值：样本量小的时候
    /// 插值会造出「没有一帧真的这么慢」的数字，回归基线比的是实际观测值。
    pub fn from_samples(samples: &[f32]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let pick = |p: f32| -> f32 {
            let n = sorted.len();
            let rank = ((p * n as f32).ceil() as usize).clamp(1, n);
            sorted[rank - 1]
        };
        Self {
            samples: sorted.len(),
            last_ms: samples[samples.len() - 1],
            p50_ms: pick(0.50),
            p95_ms: pick(0.95),
            p99_ms: pick(0.99),
            max_ms: sorted[sorted.len() - 1],
        }
    }
}

/// 固定容量的帧时间环形缓冲：满了就覆盖最旧的样本。
#[derive(Debug, Clone)]
pub struct FrameTimeWindow {
    buf: Vec<f32>,
    next: usize,
    /// 最近一次采到的帧时间。
    ///
    /// 不能靠 `buf.last()` 得到它：环形缓冲写满后物理顺序是「覆盖顺序」，
    /// 末元素不再是最后发生的那一帧。`last_ms` 在面板上表示「当前帧耗时」，
    /// 必须与采样时刻一致，所以单独记一份。
    last: Option<f32>,
}

impl FrameTimeWindow {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity.max(1)),
            next: 0,
            last: None,
        }
    }

    pub fn push(&mut self, ms: f32) {
        if !ms.is_finite() || ms < 0.0 {
            return;
        }
        if self.buf.len() < self.buf.capacity() {
            self.buf.push(ms);
        } else {
            let cap = self.buf.len();
            self.buf[self.next % cap] = ms;
        }
        self.next = self.next.wrapping_add(1);
        self.last = Some(ms);
    }

    pub fn snapshot(&self) -> FrameTimeStats {
        let mut stats = FrameTimeStats::from_samples(&self.buf);
        if let Some(last) = self.last {
            stats.last_ms = last;
        }
        stats
    }
}

impl Default for FrameTimeWindow {
    fn default() -> Self {
        Self::new(FRAME_TIME_WINDOW)
    }
}
