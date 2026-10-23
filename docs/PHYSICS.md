# r3e 物理系统

> 状态：**已接入**。内置最简物理后端可用（重力 / 地面碰撞 / 弹性 / 摩擦 / 休眠），
> 默认**关闭**，三层入口可开；`PhysicsBackend` 契约不变，随时可换成 Rapier。

---

## 1. 一分钟上手

```bash
# 专用物理演示场景（场景文件里已 enabled=true）
cargo run -- assets/scene_physics.json scripts/physics_demo.rhai

# 用命令行临时打开物理（覆盖场景文件设置）
cargo run -- --physics on

# 打开物理并换重力 / 地面高度
cargo run -- assets/scene_physics.json scripts/physics_demo.rhai --physics on --gravity 0,-3.0,0 --ground 0.5

# 只看不模拟（null 后端空转，脚本仍可 attach 但不产生位移）
cargo run -- --physics on --physics-backend null
```

窗口标题会实时显示物理状态：`物理 builtin / 6 刚体` 或 `物理关`。

---

## 2. 三层开关与优先级

物理配置按 **命令行 > 场景文件 > 内置默认** 合并，后两者缺席时由前一层决定：

| 层级 | 入口 | 写法 | 特点 |
| --- | --- | --- | --- |
| 命令行 | `--physics` / `--physics-backend` / `--gravity` / `--ground` | `--physics on` | 最高优先级，且**粘住**热重载（`Physics.overrides`） |
| 场景文件 | `scene.json` 的 `physics` 段 | `"physics": { "enabled": true }` | 随场景走，热重载时重新读取 |
| 脚本 | `physics_set_enabled()` / `physics_set_backend()` 等 | `physics_set_enabled(true)` | 运行时任意时刻生效，热重载后回到场景值 |
| 内置默认 | 代码常量 | `enabled=false`、`gravity=(0,-9.81,0)`、`ground=0`、后端 `null` | 缺省即「与物理未接入时的行为完全一致」 |

几条关键语义：

- **开启但没指定后端** → 自动升级到 `builtin`。不会出现「开关开了、后端还是 null、什么都不动」的困惑。
- **关闭时**：`step` 直接返回、`attach_body` 返回 `false`、`body_count()` 恒为 `0`，物件完全由脚本驱动。
- **命令行粘性**：`--physics on` 之后即使脚本热重载（重放场景 JSON，其中写的是 `enabled:false`），物理仍保持开启。
- **脚本改动是易失的**：脚本里 `physics_set_enabled(false)` 后热重载，物理状态回到场景文件声明的值。

### 场景文件写法

```json
{
  "physics": {
    "enabled": true,
    "backend": "builtin",
    "gravity": [0, -9.81, 0],
    "ground": 0.0
  },
  "objects": [
    { "name": "ground", "mesh": "plane", "scale": [30, 1, 30] },
    {
      "name": "ball", "mesh": "sphere",
      "position": [-3, 6, 0], "scale": [1, 1, 1],
      "physics": { "body": "dynamic", "mass": 1.0, "restitution": 0.85, "friction": 0.3 }
    },
    {
      "name": "pillar", "mesh": "cylinder",
      "position": [3, 2, -3], "scale": [1.2, 4, 1.2],
      "physics": { "body": "static", "collider": "cylinder" }
    }
  ]
}
```

`physics` 段全部字段可省，`physics` 键本身也可省（等于没声明）。

---

## 3. 物件级物理字段

| 字段 | 类型 | 缺省 | 说明 |
| --- | --- | --- | --- |
| `body` | string | `dynamic` | `static` / `dynamic` / `kinematic`（`fixed` 是 static 别名） |
| `collider` | string | 由 mesh 推导 | `box` / `sphere` / `capsule` / `cylinder` |
| `mass` | f32 | `1.0` | ≤1e-4 视为缺省 |
| `restitution` | f32 | `0.2` | 0 = 完全不弹，1 = 完全弹性 |
| `friction` | f32 | `0.6` | 落地时的切向速度衰减系数 |
| `half_extents` | [f32;3] | `scale * 0.5` | 显式覆盖碰撞体半尺寸 |

**只有声明了 `physics` 的物件才会被自动绑定成刚体**。没声明的（纯装饰）永远由脚本驱动。

---

## 4. 关闭物理开关的边界

| 情况 | 生效层 | 优先级 |
| --- | --- | --- |
| `--physics on` + 场景 `enabled:false` | 命令行 | 命令行赢 |
| 场景 `enabled:true` + `--physics off` | 命令行 | 命令行赢 |
| 场景 `enabled:true` + 脚本 `physics_set_enabled(false)` | 脚本 | 脚本赢（运行时） |
| 场景 `enabled:true` + 脚本关闭 + 热重载 | 场景文件 | 场景文件赢（脚本改动被丢弃） |

---

## 5. 内置后端的物理规则

`BuiltinBackend` 是纯 Rust 自实现（零外部依赖）。每帧先按 1/60 秒切子步（总时长 clamp 到 0.1s、最多 6 步），
每个子步对动态刚体做一次半隐式欧拉积分：

```text
v += g * h
p += v * h
if p.y <= ground + support:            // 只有下穿地面才求解
    p.y = ground + support
    if v.y < 0:
        if -v.y > 0.6 and restitution > 0.01:
            v.y = -v.y * restitution    // 够快才反弹，避免落地抖动
        else:
            v.y = 0
    v.x *= clamp(1 - friction*8*h)      // 切向摩擦，指数衰减，与帧率无关
    v.z *= clamp(1 - friction*8*h)
    if |v|² < 0.0025 and 持续 > 0.35s:
        sleeping = true; v = 0
else:
    v *= (1 - 0.02*h)                   // 空气阻尼，防长时间自由落体数值过大
```

| 概念 | 取值 / 规则 |
| --- | --- |
| 支撑半高 `support` | 球用 `half.x`（半径），盒/柱/胶囊用 `half.y` |
| 地面 | 无限大水平平面，高度由 `ground` 决定，默认 `y=0` |
| 反弹阈值 | 法向速度 > **0.6 m/s** 才反弹，否则直接归零（消除落地抖动） |
| 摩擦 | 切向速度乘以 `1 - friction*8*h`，`friction` 钳制到 `[0, 2]` |
| 休眠 | 速度模长 < **0.05 m/s** 持续 **0.35s** → 休眠并清零速度 |
| 唤醒 | `apply_impulse`，或脚本 `set_pos` 造成位置突变（`pull_from_scene` 检测） |
| 静态 / 运动学刚体 | `inv_mass = 0`，不参与积分；`static` 完全不动，`kinematic` 由脚本挪动后同步 |
| 质量 | 冲量按 `inv_mass = 1/mass` 折算成速度增量 |
| 节点回收 | 脚本 `remove_node` 后，`sync_to_scene` 找不到节点即自动回收刚体，无句柄泄漏 |

### 刻意不做的能力（保证 ~400 行可读）

- **物件之间不碰撞**：只与地面平面相交，球不会撞球、盒子不会压柱子。
- **无旋转动力学**：物体平移、不翻滚、不受力矩。
- **无关节、无堆叠求解、无 CCD**：不做金字塔堆叠，不防高速穿透。

需要这些能力时走第 8 节的切换路径，而不是继续往内置后端加。

---

## 6. 脚本 API

### 6.1 开关与环境

| 函数 | 返回 | 说明 |
| --- | --- | --- |
| `physics_enabled()` | bool | 当前开关状态 |
| `physics_set_enabled(on)` | bool | 返回**操作后的**开关状态 |
| `physics_backend()` | string | `null` / `builtin` / `rapier` |
| `physics_set_backend(name)` | bool | 运行时切后端，会**清空已有刚体**；`rapier` 未编译时返回 false |
| `physics_gravity()` | f64 | 取 `y` 分量 |
| `physics_set_gravity(x,y,z)` | void | 设置重力向量 |
| `physics_ground()` | f64 | 地面平面高度 |
| `physics_set_ground(y)` | void | 设置地面平面高度 |

### 6.2 刚体

| 函数 | 返回 | 说明 |
| --- | --- | --- |
| `attach_body(name, kind, mass)` | bool | 为节点创建刚体；物理关闭时返回 false；重复调用会先回收旧刚体 |
| `detach_body(name)` | bool | 销毁该节点的刚体并清空 `Node::body` |
| `body_impulse(name, x, y, z)` | bool | 施加瞬时冲量（会唤醒休眠体） |
| `body_count()` | i64 | 当前刚体数；物理关闭时恒为 0 |

`attach_body` 的碰撞形状优先取节点 `physics.collider`，没有则按 `mesh` 推导（球 → 球、柱 → 柱、其余 → 盒），
半尺寸取 `scale * 0.5`。因此脚本 `create_node` 出来的物件可以直接 attach，无需额外声明。

### 6.3 示例

```rhai
fn on_start() {
    // 场景里已 enabled=true，这里只确认环境
    log("后端=" + physics_backend() + " 重力=" + physics_gravity() + " 地面=" + physics_ground());
}

fn on_update(dt) {
    // 每 2 秒从空中丢一个球
    let slot = (time() / 2.0).floor();
    let name = "drop_" + slot.to_string();
    if !exists(name) {
        create_node(name, "sphere", rand(-6.0, 6.0), 11.0, rand(-4.0, 4.0));
        set_scale_uniform(name, rand(0.6, 1.2));
        if attach_body(name, "dynamic", 1.0) {
            body_impulse(name, rand(-3.0, 3.0), 0.0, rand(-3.0, 3.0));
        }
    }
}
```

---

## 7. 与帧循环 / 热重载的配合

```
dt → scripts.tick → camera input → sync_physics_bindings → physics.step → update_world_transforms
```

- **绑定时机**：`Physics` 维护一个 `generation`（配置世代号）。开关切换、后端重建、`clear_bodies` 都会自增；
  `app` 只在世代变化时把「场景声明了 physics 的物件」重绑一次。因此脚本手动 `attach_body` 的刚体不会被覆盖。
- **脚本每帧改位置会与物理打架**：`pull_from_scene` 会识别「脚本挪了节点」并唤醒刚体，随后物理把它拽回去。
  正确做法是物理开启时不要对同一物件 `set_pos`（`scripts/main.rhai` 里就是 `if !physics_enabled() { ... }` 包起来）。
- **热重载**：清空全部刚体 → 重放场景 JSON → 重新应用场景 physics 配置 → 重跑脚本；
  命令行的 `--physics` 等覆盖项保留（`overrides` 粘性）。

---

## 8. 换成 Rapier（或任何外部后端）

契约没有变，改动面固定在三处：

1. 新建 `crates/r3e-physics/src/rapier_backend.rs`，实现 `PhysicsBackend` 的 8 个必需方法：
   `name` / `create_body` / `destroy_body` / `apply_impulse` / `set_gravity` / `step` / `sync_to_scene` / `body_count`。
   `BodyDesc.shape` → `SharedShape`，`BodyKind` → `RigidBodyType`，`step` 内 `pipeline.step(...)`，
   `sync_to_scene` 写回 `transform.position` / `rotation`。
2. `PhysicsKind` 增加真实可用分支，`Physics::rebuild_backend` 里把 `Rapier` 指向新实例。
3. `Cargo.toml` 加 `rapier3d` 依赖（当前刻意零依赖）。

场景 JSON、脚本 API、`app` 层、`core::Scene` 都**不需要改**。
唯一语义差异：外部后端做物件间碰撞与旋转，内置后端不做 —— 这属于「换后端带来的能力升级」，不是接口破坏。

---

## 9. 已实测行为

| 场景 | 观测 |
| --- | --- |
| `assets/scene_physics.json`，默认配置 | 场景 `enabled:true` → 日志「物理: 启用 / 后端 builtin」，绑定 5 个场景刚体 |
| 同场景 `--physics off` | 日志「物理: 关闭」，不绑定任何刚体，脚本 `attach_body` 返回 false |
| `assets/scene.json`（无 physics 段）`--physics on` | 后端自动升级为 `builtin`，绑定 3 个刚体（地面 + 两柱），脚本挂上 `orbiter` 后共 4 个 |
| 下落积分 | 球从 `y=6` 落到 `y=0.5` 后反弹至 `y≈4.34`（restitution 0.85），逐次衰减 |
| 静止与休眠 | 盒子从 `y=12` 落到 `y=0.6999…`（half=0.7）后数值恒定不再抖动 |
| 运行时开关 | `physics_set_enabled(false/true)`、`physics_set_backend("null"/"builtin")` 均生效 |
| `physics_set_backend("rapier")` | 返回 false 并打印「未编译进本构建」，不静默降级 |

单元测试：`cargo test -p r3e-physics`（10 个用例，覆盖关闭语义、优先级合并、世代号、形状推导、反弹、休眠唤醒、后端切换清空）。

---

## 10. 已知限制

- 物件互不碰撞，所以「一堆球掉进箱子」只会全部穿到地面。
- 无旋转，盒子落地不会侧翻，斜坡上不会滚动。
- 地面是无限平面，没有地形 / 网格碰撞。
- 高速度（>~50 m/s）可能单帧穿过地面（无 CCD）。
- 物理与脚本共用 `RefCell<Scene>`，脚本里不能长期持有 `borrow()` 期间再 `attach_body`（现有 API 已规避）。
