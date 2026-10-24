# r3e 物理系统

> 状态：**已接入两个后端**。内置最简后端（重力 / 地面碰撞 / 弹性 / 摩擦 / 休眠，零依赖）+
> Rapier3D 后端（物件互撞、堆叠、旋转、碰撞事件，由 `--features rapier` 隔离）。
> 默认**关闭**、默认后端仍是 `builtin`；后端可运行时切换，`PhysicsBackend` 契约对上层零变化。

---

## 1. 一分钟上手

```bash
# 专用物理演示场景（场景文件里已 enabled=true）
cargo run -- assets/scene_physics.json scripts/physics_demo.rhai

# Rapier 演示：堆叠方块 + 碰撞事件（必须先带 feature 编译，否则 rapier 不可选）
cargo run --features rapier -- assets/scene_rapier.json scripts/rapier_demo.rhai

# 后端运行时切换演示：3s 降 builtin、6s 升回 rapier，观察堆叠行为与重绑日志
cargo run --features rapier -- assets/scene_rapier.json scripts/backend_switch_demo.rhai

# 用命令行临时打开物理（覆盖场景文件设置）
cargo run -- --physics on

# 打开物理并换重力 / 地面高度
cargo run -- assets/scene_physics.json scripts/physics_demo.rhai --physics on --gravity 0,-3.0,0 --ground 0.5

# 只看不模拟（null 后端空转，脚本仍可 attach 但不产生位移）
cargo run -- --physics on --physics-backend null

# 带 rapier 构建时用命令行指定后端
cargo run --features rapier -- assets/scene_physics.json scripts/physics_demo.rhai --physics on --physics-backend rapier
```

窗口标题会实时显示物理状态：`物理 rapier / 6 刚体` 或 `物理关`。

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
| `physics_capabilities()` | string | 当前后端能力位，逗号分隔（见 7.3） |
| `physics_has_capability(name)` | bool | 单个能力位查询，脚本据此分支而非硬编码后端名 |
| `physics_set_ccd(on)` | bool | **恒返回 false** 并打印「未支持」：契约里还没有 CCD 开关，能力位 `ccd=false` |
| `physics_take_events()` | array | 取走本帧碰撞事件（每条形如 `started:2-3`），无 events 能力时恒为空数组 |
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

## 7. Rapier 后端

### 7.1 编译与启用

Rapier 走 **optional feature**，默认构建完全不含它（`cargo build` 不拉 `rapier3d` / `parry3d` / `nalgebra`）：

```toml
# crates/r3e-physics/Cargo.toml
rapier = ["dep:rapier3d"]

# crates/r3e-app/Cargo.toml —— 透传，方便直接 cargo run --features rapier
rapier = ["r3e-physics/rapier"]
```

| 构建方式 | `builtin` | `rapier` | 说明 |
| --- | --- | --- | --- |
| `cargo run` | 可用 | **不可用**（`set_kind(Rapier)` 报错，`set_backend("rapier")` 返回 false） | 默认构建，二进制不带物理引擎 |
| `cargo run --features rapier` | 可用 | 可用 | 两后端同在一个二进制里，可运行时互切 |

「未编译就明确报错、不静默降级」是刻意行为：`PhysicsKind::available_names()` 会按当前构建返回 `builtin / null` 或 `builtin / null / rapier`，
命令行帮助、未知后端提示、脚本报错都用它，避免用户以为切了后端其实没切。

### 7.2 与内置后端的行为差异

| 行为 | `builtin` | `rapier` |
| --- | --- | --- |
| 刚体 vs 地面 | 有（无限平面） | 有（`halfspace`，随 `ground` 平移） |
| 刚体 vs 刚体 | **无** | 有，可堆叠、可互相推挤 |
| 旋转 / 翻滚 / 力矩 | 无 | 有，`sync_to_scene` 写回 `transform.rotation` |
| 碰撞事件 | 无 | 有（`started` / `stopped`） |
| CCD（高速防穿透） | 无 | 底层有能力，门面未暴露 → 能力位 `false`，高速物体仍可能穿透 |
| 关节 | 无 | 底层支持，门面暂不暴露 |
| 子步策略 | 1/60 切分，最多 6 步 | 同左，`MAX_SUBSTEPS = 6` 保持一致节奏 |
| 静态 `plane` | 装饰用，碰撞由 `ground` 平面负责 | 同左（不做网格碰撞） |

对齐过的语义（换后端不会让脚本行为跳变）：

- `Static` 完全不动：脚本挪静态物件不会带动碰撞，需重建刚体。
- `Kinematic` 每帧从节点推位姿给物理引擎，由它带着动态体走。
- `Dynamic` 由物理积分；脚本 `set_pos` 位移超过 `1e-4` 视为**瞬移**，位置以节点为准并清零线速度/角速度。
- 节点被 `remove_node` → `sync_to_scene` 回收刚体（`records` 里查不到节点即销毁），无句柄泄漏。
- 地面本身不是场景节点，事件里以 `none` 表示（例如 `started:none-9` 是 9 号节点砸地）。

### 7.3 能力位

`PhysicsBackend::capabilities()` 用 5 个布尔位申报真实能力，**这是脚本做兼容分支的唯一正确姿势**：

| 位 / 脚本名 | builtin | rapier |
| --- | --- | --- |
| `body_body`（`body_body_contacts`） | false | true |
| `rotation`（`rotational_dynamics`） | false | true |
| `joints` | false | **false** |
| `ccd` | false | **false** |
| `events`（`collision_events`） | false | true |

申报口径：**能力位只报门面真能用到的能力**（`BackendCaps::EXPOSED`）。Rapier 底层确实有
`ccd_solver` / `impulse_joints` / `multibody_joints`，但 `PhysicsBackend` 契约里既没有打开 CCD 的
开关、也没有建关节的方法，脚本更没有任何入口——这种情况下报 `true` 就是虚报，脚本会拿着
`physics_has_capability("joints") == true` 去调用并不存在的接口。所以这两位一律为 `false`，
等门面补齐接口（trait 方法 + 脚本入口 + 契约测试）后再一起点亮。
`BackendCaps::CONTACT` 是当前上限档，`BackendCaps::FULL`（含 joints/ccd）只是给未来预留的档位，
`tests/backend_contract.rs::no_backend_overclaims_capabilities` 会拦住任何越界申报。

脚本侧：`physics_capabilities()` 拿整串（全 false 时为 `none`），`physics_has_capability("events")` 查单个。
**不要在脚本里判断 `physics_backend() == "rapier"`** —— 那会把「后端名」和「能力」绑死，将来接入第三个后端就又要改脚本。

### 7.4 运行时切换

`physics_set_backend("builtin")` / `("rapier")` / `("null")`，语义与 `set_kind` 一致：

1. 清空**全部**刚体并 bump `generation`；
2. `app` 在下一代重新把「场景声明了 physics 的物件」绑成刚体；
3. 脚本此前 `attach_body` 的手工刚体不会自动恢复，需要脚本在 `generation` 变化后自行重绑。

切换是「换引擎」而非「改参数」，飞行中的物件会回到场景初始位姿重新下落 —— 这是预期行为，文档与日志都如实写明。

---

## 8. 与帧循环 / 热重载的配合

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

## 9. 再接入第三个后端（或替换 Rapier）

契约没有变，改动面固定在三处 —— Rapier 就是照这个模板加进来的：

1. 新建 `crates/r3e-physics/src/xxx_backend.rs`，实现 `PhysicsBackend` 的必需方法：
   `name` / `create_body` / `destroy_body` / `apply_impulse` / `set_gravity` / `step` / `sync_to_scene` / `body_count`；
   若该引擎能出事件，再实现 `drain_events` 并在 `capabilities().collision_events` 如实置位。
   **能力位只报门面真能用到的能力**：契约里没有对应方法、脚本没有入口的位（目前是 `joints` / `ccd`）
   一律置 `false`，否则脚本会拿到用不了的承诺。
2. `PhysicsKind` 增加分支 + `Physics::rebuild_backend` 指向新实例 + 在 `available_names` 里按 feature 暴露。
3. `Cargo.toml` 加 optional 依赖与 feature，`r3e-app` 透传同名 feature。

场景 JSON、脚本 API、`app` 层、`core::Scene` 都**不需要改**。

**唯一红线**：外部引擎的类型只能出现在那一个后端文件里。门面、`app`、脚本层只允许出现 `glam::Vec3` / `Quat`
和 `PhysicsBackend` 契约类型；高级能力通过 `BackendCaps` 位申报，不往 trait 里塞后端专属方法。
守不住这条，第二次切换就要动上层代码，可插拔就名存实亡。

回归口径：`crates/r3e-physics/tests/backend_contract.rs` 是一组**后端无关**的契约测试，
对当前构建里所有 `available_names()` 逐个跑（落体、销毁回收、切换清空、能力矩阵自洽），
Rapier 专属的堆叠 + 碰撞事件用例单独用 `#[cfg(feature = "rapier")]` 关起来。加新后端时把名字塞进列表即可复用。

---

## 10. 已实测行为

| 场景 | 观测 |
| --- | --- |
| `assets/scene_physics.json`，默认配置 | 场景 `enabled:true` → 日志「物理: 启用 / 后端 builtin」，绑定 5 个场景刚体 |
| 同场景 `--physics off` | 日志「物理: 关闭」，不绑定任何刚体，脚本 `attach_body` 返回 false |
| `assets/scene.json`（无 physics 段）`--physics on` | 后端自动升级为 `builtin`，绑定 3 个刚体（地面 + 两柱），脚本挂上 `orbiter` 后共 4 个 |
| 下落积分 | 球从 `y=6` 落到 `y=0.5` 后反弹至 `y≈4.34`（restitution 0.85），逐次衰减 |
| 静止与休眠 | 盒子从 `y=12` 落到 `y=0.6999…`（half=0.7）后数值恒定不再抖动 |
| 运行时开关 | `physics_set_enabled(false/true)`、`physics_set_backend("null"/"builtin")` 均生效 |
| 默认构建 `physics_set_backend("rapier")` | 返回 false 并打印「未编译进本构建」，不静默降级 |
| `cargo run --features rapier -- assets/scene_rapier.json scripts/rapier_demo.rhai` | 日志「物理: 启用 / 后端 rapier / 能力 [body_body,rotation,events]」，绑定 6 个场景刚体；脚本打印碰撞事件（如 `started:none-9` 砸地、`stopped:10-11` 方块分离），方块在平台上堆住不倒 |
| `scripts/backend_switch_demo.rhai`（同场景，运行中切换） | 3s `physics_set_backend("builtin")` → true、刚体数归 0、能力变 `none`，引擎打印「物理后端 -> builtin」并重新绑定 6 个场景刚体；6s 切回 rapier 同样成功，碰撞事件恢复 |

测试：`cargo test --workspace` 全绿；`cargo test -p r3e-physics --features rapier` 额外跑 Rapier 契约用例
（落体、销毁回收、后端切换清空、能力矩阵，以及 `rapier_stacks_bodies_and_reports_collision_events`
——球落平台触发真实互撞事件）。

---

## 11. 已知限制

按后端分列，都属于「该后端的定位」，不是待修 bug：

**`builtin`（刻意保持 ~400 行可读）**

- 物件互不碰撞，所以「一堆球掉进箱子」只会全部穿到地面。
- 无旋转，盒子落地不会侧翻，斜坡上不会滚动。
- 高速度（>~50 m/s）可能单帧穿过地面（无 CCD）。

**`rapier`**

- 关节（joint）底层支持但门面未暴露，脚本暂不能建约束，能力位 `joints=false`。
- CCD 底层有 `ccd_solver`，但契约里没有开关，能力位 `ccd=false`，高速物体仍可能穿透；脚本调
  `physics_set_ccd()` 会拿到 `false` 与「未支持」日志。
- 静态 `plane` 只是装饰，物理地面永远是 `ground` 定义的无限平面，没有地形 / 网格碰撞。

**通用**

- 物理与脚本共用 `RefCell<Scene>`，脚本里不能长期持有 `borrow()` 期间再 `attach_body`（现有 API 已规避）。
- 切换后端会清空全部刚体并让物件回到场景初始位姿，运行中「换引擎」不是无损操作。
