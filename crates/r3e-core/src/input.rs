//! 输入状态：键盘按键、鼠标按键 / 位置 / 滚轮，以及手柄按键 / 摇杆的帧级快照。
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
    "left_x", "left_y", "right_x", "right_y", "lt", "rt", "unused6", "unused7",
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

/// 键盘按键总量。`KeyCode` 的判别值即数组下标，最后一个变体必须是 `KEY_COUNT - 1`。
pub const KEY_COUNT: usize = 98;

/// 引擎自有的键盘键位枚举。
///
/// 刻意不引用 winit 的 `Key` / `KeyCode`：core 层不依赖窗口 crate，
/// 设备侧的物理/逻辑键到本枚举的映射由 app 层完成（见 `app.rs::key_code_of`）。
/// 判别值即状态数组下标，顺序不可随意调整（`KEY_NAMES` 与之一一对应）。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    A = 0,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Space,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    SuperLeft,
    SuperRight,
    Minus,
    Equal,
    BracketLeft,
    BracketRight,
    Backslash,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,
    Backquote,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadAdd,
    NumpadSubtract,
    NumpadMultiply,
    NumpadDivide,
    NumpadDecimal,
    CapsLock,
}

impl KeyCode {
    /// 数组下标（= 判别值）。
    pub fn index(self) -> usize {
        self as usize
    }

    /// 下标 → 枚举；越界返回 `None`。
    pub fn from_index(index: usize) -> Option<KeyCode> {
        ALL_KEYS.get(index).copied()
    }

    /// 规范化名字（脚本 API、日志用）。
    pub fn name(self) -> &'static str {
        KEY_NAMES[self.index()]
    }

    /// 是否小键盘键（数字 / 运算符）。用于把物理小键盘与主键区区分开。
    pub fn is_numpad(self) -> bool {
        matches!(
            self,
            KeyCode::Numpad0
                | KeyCode::Numpad1
                | KeyCode::Numpad2
                | KeyCode::Numpad3
                | KeyCode::Numpad4
                | KeyCode::Numpad5
                | KeyCode::Numpad6
                | KeyCode::Numpad7
                | KeyCode::Numpad8
                | KeyCode::Numpad9
                | KeyCode::NumpadAdd
                | KeyCode::NumpadSubtract
                | KeyCode::NumpadMultiply
                | KeyCode::NumpadDivide
                | KeyCode::NumpadDecimal
        )
    }

    /// 是否修饰键（Shift / Ctrl / Alt / Super）。
    pub fn is_modifier(self) -> bool {
        matches!(
            self,
            KeyCode::ShiftLeft
                | KeyCode::ShiftRight
                | KeyCode::ControlLeft
                | KeyCode::ControlRight
                | KeyCode::AltLeft
                | KeyCode::AltRight
                | KeyCode::SuperLeft
                | KeyCode::SuperRight
        )
    }
}

/// 全部键位，下标与判别值严格一致（`from_index` 用，避免 unsafe transmute）。
const ALL_KEYS: [KeyCode; KEY_COUNT] = [
    KeyCode::A,
    KeyCode::B,
    KeyCode::C,
    KeyCode::D,
    KeyCode::E,
    KeyCode::F,
    KeyCode::G,
    KeyCode::H,
    KeyCode::I,
    KeyCode::J,
    KeyCode::K,
    KeyCode::L,
    KeyCode::M,
    KeyCode::N,
    KeyCode::O,
    KeyCode::P,
    KeyCode::Q,
    KeyCode::R,
    KeyCode::S,
    KeyCode::T,
    KeyCode::U,
    KeyCode::V,
    KeyCode::W,
    KeyCode::X,
    KeyCode::Y,
    KeyCode::Z,
    KeyCode::Digit0,
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
    KeyCode::F1,
    KeyCode::F2,
    KeyCode::F3,
    KeyCode::F4,
    KeyCode::F5,
    KeyCode::F6,
    KeyCode::F7,
    KeyCode::F8,
    KeyCode::F9,
    KeyCode::F10,
    KeyCode::F11,
    KeyCode::F12,
    KeyCode::ArrowUp,
    KeyCode::ArrowDown,
    KeyCode::ArrowLeft,
    KeyCode::ArrowRight,
    KeyCode::Space,
    KeyCode::Enter,
    KeyCode::Escape,
    KeyCode::Tab,
    KeyCode::Backspace,
    KeyCode::Delete,
    KeyCode::Insert,
    KeyCode::Home,
    KeyCode::End,
    KeyCode::PageUp,
    KeyCode::PageDown,
    KeyCode::ShiftLeft,
    KeyCode::ShiftRight,
    KeyCode::ControlLeft,
    KeyCode::ControlRight,
    KeyCode::AltLeft,
    KeyCode::AltRight,
    KeyCode::SuperLeft,
    KeyCode::SuperRight,
    KeyCode::Minus,
    KeyCode::Equal,
    KeyCode::BracketLeft,
    KeyCode::BracketRight,
    KeyCode::Backslash,
    KeyCode::Semicolon,
    KeyCode::Quote,
    KeyCode::Comma,
    KeyCode::Period,
    KeyCode::Slash,
    KeyCode::Backquote,
    KeyCode::Numpad0,
    KeyCode::Numpad1,
    KeyCode::Numpad2,
    KeyCode::Numpad3,
    KeyCode::Numpad4,
    KeyCode::Numpad5,
    KeyCode::Numpad6,
    KeyCode::Numpad7,
    KeyCode::Numpad8,
    KeyCode::Numpad9,
    KeyCode::NumpadAdd,
    KeyCode::NumpadSubtract,
    KeyCode::NumpadMultiply,
    KeyCode::NumpadDivide,
    KeyCode::NumpadDecimal,
    KeyCode::CapsLock,
];

/// 与 `KeyCode` 判别值严格同序的名字表；脚本里按名查询用，也是 `parse_key` 的第一层查找表。
const KEY_NAMES: [&str; KEY_COUNT] = [
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    "up",
    "down",
    "left",
    "right",
    "space",
    "enter",
    "escape",
    "tab",
    "backspace",
    "delete",
    "insert",
    "home",
    "end",
    "pageup",
    "pagedown",
    "shift_left",
    "shift_right",
    "ctrl_left",
    "ctrl_right",
    "alt_left",
    "alt_right",
    "super_left",
    "super_right",
    "minus",
    "equal",
    "bracket_left",
    "bracket_right",
    "backslash",
    "semicolon",
    "quote",
    "comma",
    "period",
    "slash",
    "backquote",
    "numpad0",
    "numpad1",
    "numpad2",
    "numpad3",
    "numpad4",
    "numpad5",
    "numpad6",
    "numpad7",
    "numpad8",
    "numpad9",
    "numpad_add",
    "numpad_subtract",
    "numpad_multiply",
    "numpad_divide",
    "numpad_decimal",
    "caps_lock",
];

/// 单个可打印字符 → 键位。字母不区分大小写，US 布局上的 Shift 符号归到数字键。
pub fn key_from_char(c: char) -> Option<KeyCode> {
    let index = match c.to_ascii_lowercase() {
        'a'..='z' => (c.to_ascii_lowercase() as usize) - ('a' as usize) + KeyCode::A.index(),
        '0'..='9' => (c as usize) - ('0' as usize) + KeyCode::Digit0.index(),
        // Shift 后的符号落到同一个数字键上
        '!' => KeyCode::Digit1.index(),
        '@' => KeyCode::Digit2.index(),
        '#' => KeyCode::Digit3.index(),
        '$' => KeyCode::Digit4.index(),
        '%' => KeyCode::Digit5.index(),
        '^' => KeyCode::Digit6.index(),
        '&' => KeyCode::Digit7.index(),
        '*' => KeyCode::Digit8.index(),
        '(' => KeyCode::Digit9.index(),
        ')' => KeyCode::Digit0.index(),
        '-' | '_' => KeyCode::Minus.index(),
        '=' | '+' => KeyCode::Equal.index(),
        '[' | '{' => KeyCode::BracketLeft.index(),
        ']' | '}' => KeyCode::BracketRight.index(),
        '\\' | '|' => KeyCode::Backslash.index(),
        ';' | ':' => KeyCode::Semicolon.index(),
        '\'' | '"' => KeyCode::Quote.index(),
        ',' | '<' => KeyCode::Comma.index(),
        '.' | '>' => KeyCode::Period.index(),
        '/' | '?' => KeyCode::Slash.index(),
        '`' | '~' => KeyCode::Backquote.index(),
        ' ' => KeyCode::Space.index(),
        _ => return None,
    };
    KeyCode::from_index(index)
}

/// 键位名 → 枚举。支持规范名、常见别名与单个字符（`"w"` / `"W"` / `"!"` / `" "`）。
pub fn parse_key(name: &str) -> Option<KeyCode> {
    let key = name.trim().to_ascii_lowercase();
    if key.is_empty() {
        return None;
    }
    if let Some(pos) = KEY_NAMES.iter().position(|n| *n == key) {
        return KeyCode::from_index(pos);
    }
    let alias = match key.as_str() {
        "esc" => KeyCode::Escape,
        "return" | "cr" => KeyCode::Enter,
        "del" => KeyCode::Delete,
        "ins" => KeyCode::Insert,
        "pgup" | "page_up" => KeyCode::PageUp,
        "pgdn" | "page_down" => KeyCode::PageDown,
        "arrow_up" => KeyCode::ArrowUp,
        "arrow_down" => KeyCode::ArrowDown,
        "arrow_left" => KeyCode::ArrowLeft,
        "arrow_right" => KeyCode::ArrowRight,
        "shift" | "lshift" | "shiftleft" | "left_shift" => KeyCode::ShiftLeft,
        "rshift" | "shiftright" | "right_shift" => KeyCode::ShiftRight,
        "ctrl" | "control" | "lctrl" | "ctrlleft" | "left_ctrl" | "left_control" => {
            KeyCode::ControlLeft
        }
        "rctrl" | "ctrlright" | "right_ctrl" | "right_control" => KeyCode::ControlRight,
        "alt" | "lalt" | "altleft" | "left_alt" | "option" => KeyCode::AltLeft,
        "ralt" | "altright" | "right_alt" | "altgr" => KeyCode::AltRight,
        "super" | "meta" | "cmd" | "command" | "win" | "lwin" | "superleft" | "left_super"
        | "left_meta" => KeyCode::SuperLeft,
        "rwin" | "superright" | "right_super" | "right_meta" => KeyCode::SuperRight,
        "capslock" | "caps" => KeyCode::CapsLock,
        "numpad_plus" | "add" => KeyCode::NumpadAdd,
        "numpad_minus" | "subtract" => KeyCode::NumpadSubtract,
        "numpad_mul" | "multiply" => KeyCode::NumpadMultiply,
        "numpad_div" | "divide" => KeyCode::NumpadDivide,
        "numpad_dot" | "numpad_point" => KeyCode::NumpadDecimal,
        "num0" => KeyCode::Numpad0,
        "num1" => KeyCode::Numpad1,
        "num2" => KeyCode::Numpad2,
        "num3" => KeyCode::Numpad3,
        "num4" => KeyCode::Numpad4,
        "num5" => KeyCode::Numpad5,
        "num6" => KeyCode::Numpad6,
        "num7" => KeyCode::Numpad7,
        "num8" => KeyCode::Numpad8,
        "num9" => KeyCode::Numpad9,
        _ => {
            // 单字符（含空格）走字符表
            let mut chars = key.chars();
            let (c, rest) = (chars.next()?, chars.next());
            if rest.is_some() {
                return None;
            }
            return key_from_char(c);
        }
    };
    Some(alias)
}

/// 键盘状态：98 个键的持续 / 边沿状态。
#[derive(Debug, Clone)]
pub struct KeyboardState {
    keys: [ButtonEdge; KEY_COUNT],
}

impl Default for KeyboardState {
    fn default() -> Self {
        Self {
            keys: [ButtonEdge::default(); KEY_COUNT],
        }
    }
}

impl KeyboardState {
    /// 写入新状态；仅在状态翻转时记录边沿（重复的按下事件不会重复触发）。
    pub fn set(&mut self, key: KeyCode, down: bool) {
        if let Some(slot) = self.keys.get_mut(key.index()) {
            slot.set(down);
        }
    }

    pub fn down(&self, key: KeyCode) -> bool {
        self.keys.get(key.index()).map(|b| b.down).unwrap_or(false)
    }

    pub fn pressed(&self, key: KeyCode) -> bool {
        self.keys
            .get(key.index())
            .map(|b| b.pressed)
            .unwrap_or(false)
    }

    pub fn released(&self, key: KeyCode) -> bool {
        self.keys
            .get(key.index())
            .map(|b| b.released)
            .unwrap_or(false)
    }

    /// 是否有任意键处于按住状态（相机输入的快路径判据）。
    pub fn any_down(&self) -> bool {
        self.keys.iter().any(|b| b.down)
    }

    /// 是否有任意键在本帧刚按下。
    pub fn any_pressed(&self) -> bool {
        self.keys.iter().any(|b| b.pressed)
    }

    /// 当前按住的键，按枚举顺序返回（脚本 / HUD 用）。
    pub fn held(&self) -> Vec<KeyCode> {
        self.keys
            .iter()
            .enumerate()
            .filter(|(_, b)| b.down)
            .filter_map(|(i, _)| KeyCode::from_index(i))
            .collect()
    }

    pub fn held_count(&self) -> usize {
        self.keys.iter().filter(|b| b.down).count()
    }

    /// 帧末清理边沿，`down` 保留。
    pub fn clear_frame(&mut self) {
        for b in &mut self.keys {
            b.clear_frame();
        }
    }

    /// 失焦时整体复位：持续状态与边沿一起清掉，避免切窗口后卡键。
    pub fn reset(&mut self) {
        *self = Self::default();
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
    pub keyboard: KeyboardState,
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
        self.keyboard.clear_frame();
    }

    /// 窗口失焦或手柄断开时清空持续状态，避免"粘键"。
    pub fn reset_held(&mut self) {
        for b in &mut self.mouse {
            b.set(false);
        }
        for b in &mut self.pad.buttons {
            b.set(false);
        }
        self.keyboard.reset();
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

    /// 键盘便捷查询：持续按住。
    pub fn key_down(&self, key: KeyCode) -> bool {
        self.keyboard.down(key)
    }

    /// 键盘便捷查询：本帧刚按下。
    pub fn key_pressed(&self, key: KeyCode) -> bool {
        self.keyboard.pressed(key)
    }

    /// 键盘便捷查询：本帧刚松开。
    pub fn key_released(&self, key: KeyCode) -> bool {
        self.keyboard.released(key)
    }

    pub fn pad_down(&self, index: usize) -> bool {
        self.pad.buttons.get(index).map(|b| b.down).unwrap_or(false)
    }

    pub fn pad_pressed(&self, index: usize) -> bool {
        self.pad
            .buttons
            .get(index)
            .map(|b| b.pressed)
            .unwrap_or(false)
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
        .or(match key.as_str() {
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
    PAD_AXIS_NAMES.iter().position(|n| *n == key).or({
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

#[cfg(test)]
mod keyboard_tests {
    use super::*;

    /// 键位表自洽：枚举判别值与下标表、名字表长度一致，CapsLock 是最后一个。
    #[test]
    fn key_table_is_consistent() {
        assert_eq!(ALL_KEYS.len(), KEY_COUNT);
        assert_eq!(KEY_NAMES.len(), KEY_COUNT);
        for (i, key) in ALL_KEYS.iter().enumerate() {
            assert_eq!(key.index(), i, "{} 的判别值与下标不一致", key.name());
        }
        assert_eq!(KeyCode::CapsLock.index(), KEY_COUNT - 1);
        assert!(KeyCode::from_index(KEY_COUNT).is_none());
        // 名字唯一
        let mut names: Vec<&str> = KEY_NAMES.to_vec();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "键位名应唯一");
    }

    /// 脚本侧按键名解析：规范名、别名、单字符都要命中同一键位。
    #[test]
    fn parse_key_resolves_names_and_aliases() {
        assert_eq!(parse_key("w"), Some(KeyCode::W));
        assert_eq!(parse_key("W"), Some(KeyCode::W));
        assert_eq!(parse_key(" space "), Some(KeyCode::Space));
        assert_eq!(parse_key("esc"), Some(KeyCode::Escape));
        assert_eq!(parse_key("Return"), Some(KeyCode::Enter));
        assert_eq!(parse_key("shift"), Some(KeyCode::ShiftLeft));
        assert_eq!(parse_key("rshift"), Some(KeyCode::ShiftRight));
        assert_eq!(parse_key("f12"), Some(KeyCode::F12));
        assert_eq!(parse_key("arrow_left"), Some(KeyCode::ArrowLeft));
        assert_eq!(parse_key("num9"), Some(KeyCode::Numpad9));
        // 单字符与 Shift 符号
        assert_eq!(parse_key("!"), Some(KeyCode::Digit1));
        assert_eq!(parse_key("-"), Some(KeyCode::Minus));
        assert_eq!(parse_key("+"), Some(KeyCode::Equal));
        // 未知 / 空
        assert_eq!(parse_key("不存在的键"), None);
        assert_eq!(parse_key(""), None);
        assert_eq!(parse_key("ab"), None);
    }

    /// 每个键位名都能被 parse_key 反解回来（脚本 API 的对称性保障）。
    #[test]
    fn key_names_round_trip() {
        for key in ALL_KEYS {
            assert_eq!(parse_key(key.name()), Some(key), "{} 无法反解", key.name());
        }
    }

    /// 按下 → 按住 → 抬起 三态。
    #[test]
    fn key_three_states() {
        let mut kb = KeyboardState::default();
        assert!(!kb.down(KeyCode::W));

        kb.set(KeyCode::W, true);
        assert!(kb.down(KeyCode::W), "按下后应持续按住");
        assert!(kb.pressed(KeyCode::W), "按下当帧应有按下边沿");
        assert!(!kb.released(KeyCode::W));

        // 下一帧：按住状态保持，边沿消失
        kb.clear_frame();
        assert!(kb.down(KeyCode::W));
        assert!(!kb.pressed(KeyCode::W));

        kb.set(KeyCode::W, false);
        assert!(!kb.down(KeyCode::W), "抬起后不应再按住");
        assert!(kb.released(KeyCode::W), "抬起当帧应有抬起边沿");
        assert!(!kb.pressed(KeyCode::W));

        kb.clear_frame();
        assert!(!kb.released(KeyCode::W));
        assert!(!kb.down(KeyCode::W));
    }

    /// 单帧边沿只触发一次：重复的按下事件不重复产生 pressed。
    #[test]
    fn key_edge_fires_once_per_press() {
        let mut kb = KeyboardState::default();
        kb.set(KeyCode::Space, true);
        assert!(kb.pressed(KeyCode::Space));
        assert!(kb.any_pressed());

        // 系统自动重复（repeat）会再送一次按下：不应再次触发
        kb.set(KeyCode::Space, true);
        assert!(kb.pressed(KeyCode::Space), "同帧重复按下仍属同一帧边沿");

        kb.clear_frame();
        kb.set(KeyCode::Space, true);
        assert!(!kb.pressed(KeyCode::Space), "跨帧重复按下不应再次触发边沿");
        assert!(!kb.any_pressed());
        assert!(kb.down(KeyCode::Space), "持续状态不受边沿清空影响");
    }

    /// 失焦清空：持续状态与未消费的边沿一起清掉。
    #[test]
    fn reset_clears_held_and_edges() {
        let mut kb = KeyboardState::default();
        kb.set(KeyCode::W, true);
        kb.set(KeyCode::ShiftLeft, true);
        assert_eq!(kb.held_count(), 2);
        assert!(kb.any_pressed());

        kb.reset();
        assert!(!kb.any_down());
        assert!(!kb.any_pressed());
        assert_eq!(kb.held_count(), 0);
        assert!(kb.held().is_empty());
        assert!(!kb.down(KeyCode::W));
        assert!(!kb.pressed(KeyCode::W));
        assert!(!kb.released(KeyCode::W));
    }

    /// held() 按枚举顺序返回按住的键，供 HUD / 脚本枚举。
    #[test]
    fn held_lists_pressed_keys_in_order() {
        let mut kb = KeyboardState::default();
        kb.set(KeyCode::Space, true);
        kb.set(KeyCode::A, true);
        kb.set(KeyCode::D, true);
        assert_eq!(
            kb.held(),
            vec![KeyCode::A, KeyCode::D, KeyCode::Space],
            "应按枚举顺序返回"
        );
    }

    /// InputState 层面的接线：end_frame 清键盘边沿、reset_held 清键盘持续状态。
    #[test]
    fn input_state_wires_keyboard() {
        let mut input = InputState::default();
        input.keyboard.set(KeyCode::E, true);
        assert!(input.key_down(KeyCode::E));
        assert!(input.key_pressed(KeyCode::E));

        input.end_frame();
        assert!(input.key_down(KeyCode::E));
        assert!(!input.key_pressed(KeyCode::E));

        input.reset_held();
        assert!(!input.key_down(KeyCode::E));
        assert!(!input.key_released(KeyCode::E), "失焦不应伪造抬起边沿");
    }
}
