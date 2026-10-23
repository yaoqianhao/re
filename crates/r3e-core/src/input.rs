//! 输入状态：鼠标按键 / 位置 / 滚轮，以及手柄按键 / 摇杆的帧级快照。
//!
//! 放在 core 层是因为脚本层要读它，而脚本层不依赖窗口或设备 crate；
//! 设备侧（winit 事件、gilrs 轮询）由 app 层写入，其余层只读。
//!
//! 边沿语义：`down` 是持续状态，`pressed` / `released` 只在发生变化的那个
//! 逻辑帧内为 true，帧末由 [`InputState::end_frame`] 清掉。

/// 鼠标按键索引。
pub const MOUSE_LEFT: usize = 0;
pub const MOUSE_RIGHT: usize = 1;
pub const MOUSE_MIDDLE: usize = 2;
pub const MOUSE_BACK: usize = 3;
pub const MOUSE_FORWARD: usize = 4;
pub const MOUSE_BUTTON_COUNT: usize = 8;

/// 手柄按键索引。命名沿用 Xbox 布局，PS 手柄按位置对应。
pub const PAD_A: usize = 0;
pub const PAD_B: usize = 1;
pub const PAD_X: usize = 2;
pub const PAD_Y: usize = 3;
pub const PAD_LB: usize = 4;
pub const PAD_RB: usize = 5;
pub const PAD_BACK: usize = 6;
pub const PAD_START: usize = 7;
pub const PAD_GUIDE: usize = 8;
pub const PAD_LS: usize = 9;
pub const PAD_RS: usize = 10;
pub const PAD_DPAD_UP: usize = 11;
pub const PAD_DPAD_DOWN: usize = 12;
pub const PAD_DPAD_LEFT: usize = 13;
pub const PAD_DPAD_RIGHT: usize = 14;
pub const PAD_BUTTON_COUNT: usize = 16;

/// 手柄轴索引。
pub const PAD_LEFT_X: usize = 0;
pub const PAD_LEFT_Y: usize = 1;
pub const PAD_RIGHT_X: usize = 2;
pub const PAD_RIGHT_Y: usize = 3;
pub const PAD_LT: usize = 4;
pub const PAD_RT: usize = 5;
pub const PAD_AXIS_COUNT: usize = 8;

/// 摇杆死区，低于该绝对值视为 0（避免漂移）。
pub const STICK_DEADZONE: f32 = 0.15;

const PAD_BUTTON_NAMES: [&str; PAD_BUTTON_COUNT] = [
    "a", "b", "x", "y", "lb", "rb", "back", "start", "guide", "ls", "rs", "up", "down", "left",
    "right", "unknown",
];

const PAD_AXIS_NAMES: [&str; PAD_AXIS_COUNT] = [
    "left_x",
    "left_y",
    "right_x",
    "right_y",
    "lt",
    "rt",
    "unused6",
    "unused7",
];

/// 单个按键的边沿状态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ButtonEdge {
    pub down: bool,
    pub pressed: bool,
    pub released: bool,
}

impl ButtonEdge {
    /// 写入新状态；仅在状态翻转时记录边沿。
    pub fn set(&mut self, down: bool) {
        if down == self.down {
            return;
        }
        if down {
            self.pressed = true;
        } else {
            self.released = true;
        }
        self.down = down;
    }

    /// 帧末清理边沿，`down` 保留。
    pub fn clear_frame(&mut self) {
        self.pressed = false;
        self.released = false;
    }
}

/// 手柄快照。
#[derive(Debug, Clone)]
pub struct PadState {
    pub connected: bool,
    pub name: String,
    pub buttons: [ButtonEdge; PAD_BUTTON_COUNT],
    pub axes: [f32; PAD_AXIS_COUNT],
}

impl Default for PadState {
    fn default() -> Self {
        Self {
            connected: false,
            name: String::new(),
            buttons: [ButtonEdge::default(); PAD_BUTTON_COUNT],
            axes: [0.0; PAD_AXIS_COUNT],
        }
    }
}

impl PadState {
    pub fn axis(&self, index: usize) -> f32 {
        self.axes.get(index).copied().unwrap_or(0.0)
    }

    /// 应用死区后的轴值。
    pub fn axis_deadzone(&self, index: usize) -> f32 {
        let v = self.axis(index);
        if v.abs() < STICK_DEADZONE {
            0.0
        } else {
            v
        }
    }
}

/// 一帧的完整输入快照。
#[derive(Debug, Clone, Default)]
pub struct InputState {
    /// 光标在窗口内的像素坐标（左上角原点）。
    pub mouse_x: f32,
    pub mouse_y: f32,
    /// 本帧光标位移。
    pub mouse_dx: f32,
    pub mouse_dy: f32,
    /// 本帧滚轮累计量（行）。
    pub scroll: f32,
    pub mouse: [ButtonEdge; MOUSE_BUTTON_COUNT],
    pub pad: PadState,
}

impl InputState {
    /// 帧末调用：清边沿与增量，保留持续状态。
    pub fn end_frame(&mut self) {
        self.mouse_dx = 0.0;
        self.mouse_dy = 0.0;
        self.scroll = 0.0;
        for b in &mut self.mouse {
            b.clear_frame();
        }
        for b in &mut self.pad.buttons {
            b.clear_frame();
        }
    }

    /// 窗口失焦或手柄断开时清空持续状态，避免"粘键"。
    pub fn reset_held(&mut self) {
        for b in &mut self.mouse {
            b.set(false);
        }
        for b in &mut self.pad.buttons {
            b.set(false);
        }
    }

    pub fn set_mouse_button(&mut self, index: usize, down: bool) {
        if let Some(b) = self.mouse.get_mut(index) {
            b.set(down);
        }
    }

    pub fn mouse_down(&self, index: usize) -> bool {
        self.mouse.get(index).map(|b| b.down).unwrap_or(false)
    }

    pub fn mouse_pressed(&self, index: usize) -> bool {
        self.mouse.get(index).map(|b| b.pressed).unwrap_or(false)
    }

    pub fn mouse_released(&self, index: usize) -> bool {
        self.mouse.get(index).map(|b| b.released).unwrap_or(false)
    }

    pub fn pad_down(&self, index: usize) -> bool {
        self.pad.buttons.get(index).map(|b| b.down).unwrap_or(false)
    }

    pub fn pad_pressed(&self, index: usize) -> bool {
        self.pad.buttons.get(index).map(|b| b.pressed).unwrap_or(false)
    }

    pub fn pad_released(&self, index: usize) -> bool {
        self.pad
            .buttons
            .get(index)
            .map(|b| b.released)
            .unwrap_or(false)
    }
}

/// 鼠标按键名 → 索引。支持 `left` / `lmb` / `mouse1` 等常见写法。
pub fn parse_mouse_button(name: &str) -> Option<usize> {
    match name.trim().to_ascii_lowercase().as_str() {
        "left" | "l" | "lmb" | "mouse1" | "primary" => Some(MOUSE_LEFT),
        "right" | "r" | "rmb" | "mouse2" | "secondary" => Some(MOUSE_RIGHT),
        "middle" | "m" | "mmb" | "mouse3" | "wheel" => Some(MOUSE_MIDDLE),
        "back" | "mouse4" | "x1" => Some(MOUSE_BACK),
        "forward" | "mouse5" | "x2" => Some(MOUSE_FORWARD),
        _ => None,
    }
}

/// 手柄按键名 → 索引。
pub fn parse_pad_button(name: &str) -> Option<usize> {
    let key = name.trim().to_ascii_lowercase();
    PAD_BUTTON_NAMES
        .iter()
        .position(|n| *n == key)
        .or_else(|| match key.as_str() {
            "south" | "cross" => Some(PAD_A),
            "east" | "circle" => Some(PAD_B),
            "west" | "square" => Some(PAD_X),
            "north" | "triangle" => Some(PAD_Y),
            "l1" => Some(PAD_LB),
            "r1" => Some(PAD_RB),
            "select" | "share" => Some(PAD_BACK),
            "options" | "menu" => Some(PAD_START),
            "mode" | "home" => Some(PAD_GUIDE),
            "l3" => Some(PAD_LS),
            "r3" => Some(PAD_RS),
            "dpad_up" => Some(PAD_DPAD_UP),
            "dpad_down" => Some(PAD_DPAD_DOWN),
            "dpad_left" => Some(PAD_DPAD_LEFT),
            "dpad_right" => Some(PAD_DPAD_RIGHT),
            _ => None,
        })
}

/// 手柄轴名 → 索引。
pub fn parse_pad_axis(name: &str) -> Option<usize> {
    let key = name.trim().to_ascii_lowercase();
    PAD_AXIS_NAMES.iter().position(|n| *n == key).or_else(|| {
        match key.as_str() {
            "lx" | "lstick_x" => Some(PAD_LEFT_X),
            "ly" | "lstick_y" => Some(PAD_LEFT_Y),
            "rx" | "rstick_x" => Some(PAD_RIGHT_X),
            "ry" | "rstick_y" => Some(PAD_RIGHT_Y),
            "l2" | "left_trigger" => Some(PAD_LT),
            "r2" | "right_trigger" => Some(PAD_RT),
            _ => None,
        }
    })
}

/// 鼠标按键索引 → 名字（日志用）。
pub fn mouse_button_name(index: usize) -> &'static str {
    match index {
        MOUSE_LEFT => "left",
        MOUSE_RIGHT => "right",
        MOUSE_MIDDLE => "middle",
        MOUSE_BACK => "back",
        MOUSE_FORWARD => "forward",
        _ => "unknown",
    }
}

/// 手柄按键索引 → 名字（日志 / HUD 用）。
pub fn pad_button_name(index: usize) -> &'static str {
    PAD_BUTTON_NAMES.get(index).copied().unwrap_or("unknown")
}
