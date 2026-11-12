# 带 GPU 的自建 runner：让像素闸门在 CI 里真的生效

本文只解决一件事：**把 `capture.frame` 像素回归从"本机质量门"搬进 CI，并且跳过即失败。**

托管 runner（`windows-latest`）没有显卡，`r3e` 拍图的子进程会以退出码 `2` 返回，用例按"跳过"计 passed。
于是 CI 上真正有效的只有基准图自洽性（存在 / 可解码 / 尺寸对不对），**画面漂移一道门也拦不住**。
自建 runner 就是把这块补上的唯一办法——不是"更好"，是唯一。

---

## 1. 分工：两个 job，两件事

| job | runner | 管什么 | 状态口径 |
| --- | --- | --- | --- |
| `visual` | `windows-latest`（托管） | 基准图存在、可解码、尺寸恰为 640×360；禁止 CI 内重建基准图 | 永远必跑；基准图退化即红 |
| `visual-gpu` | `[self-hosted, windows, gpu]`（自建） | 逐像素比 `capture_frame_640x360.png`，**拍不到图也红**；按后端拆两行（`vulkan` / `dx12`），**跑错后端也红** | 变量 `R3E_GPU_RUNNER=true` 时跑；未配 runner 时显示 skipped |

`skipped` 不阻塞合并，但它在 PR 页面上是可见的一行——不会伪装成绿。这比"看起来绿其实空跑"诚实得多。

## 2. 四个环境变量：谁是闸门，谁是保险，谁钉后端

| 变量 | 取值 | 语义 | 谁设 |
| --- | --- | --- | --- |
| `R3E_VISUAL_REGRESSION` | `1` | **闸门**：不开就整条用例直接返回（打印原因，计 passed） | CI 两个 job 都开；本机手动开 |
| `R3E_VISUAL_STRICT` | `1` | **保险**：开了之后，"跳过"与"闸门没开"都升级为 `panic`（红） | 只在 `visual-gpu` 里设 |
| `WGPU_BACKEND` | `vulkan` / `dx12` | **钉后端**：矩阵每个 job 只许跑在自己的后端上。拿不到就 `Err`（进程退出码 3），测试再从引擎日志的 `backend=` 字段独立复核一遍 | `visual-gpu` 矩阵按行给；本机手动验证时给 |
| `R3E_UPDATE_BASELINE` | `1` | 用当前画面覆盖基准图 | **只在开发者本机**；CI 里出现即 `exit 1` |

口径很简单：**严格模式 = "我保证这台机器有 GPU，请把结果当真"。**
于是显卡掉了、驱动崩了、窗口建不出来——全部算红，而不是算"这台机器不支持"。
没有 `R3E_VISUAL_STRICT`，自建 runner 上的门和托管 runner 上一样是摆设。

`WGPU_BACKEND` 解决的是另一个漏洞：**"要 dx12、实际跑在 vulkan"会伪装成绿**。
两道校验缺一不可——进程内 `backend_matches` 管"引擎有没有照做"，日志里的 `backend=` 管
"测试有没有看错"，只留一道都可能被一次回退悄悄绕过。名字写错（如 `WGPU_BACKEND=dx13`）也不会静默放行：
`parse_backends` 把不认识的名字留在第二个返回值里，按要求了后端却一个都没匹配上处理。

## 3. 机器前置（自建 runner 上要装什么）

| 项 | 要求 | 为什么 |
| --- | --- | --- |
| Git | 任意近期版本 | `actions/checkout` 依赖 |
| rustup + 工具链 | 按仓库 `rust-toolchain.toml`（`stable-x86_64-pc-windows-gnu`） | CI 不指定版本，跟着仓库走，避免双版本漂移 |
| MinGW-w64 | `x86_64-w64-mingw32-gcc` 在 `PATH` 里 | 本工程用 MinGW 链接；缺了在 `visual-gpu` 的环境自检步骤当场红 |
| 显卡 | 一块能跑 Vulkan **和** DX12 的 GPU，驱动保持更新 | 矩阵两个后端都要真跑；基线是逐像素的，驱动换了大版本可能要重建基线 |
| 会话类型 | **必须是交互式桌面会话**（见下节） | `wgpu` 要建真窗口，Session 0 的服务建不出来 |

`visual-gpu` 的第一步是"环境自检"：打印显卡型号与驱动版本、检查 `cargo` 与 `x86_64-w64-mingw32-gcc`、
检查基准图存在，并且拒绝 `R3E_UPDATE_BASELINE`。缺任何一项都当场红——不会等到跑测试才莫名其妙地挂。

## 4. 注册 runner

在 GitHub 仓库页：`Settings` → `Actions` → `Runners` → `New self-hosted runner` → `Windows` / `x64`，
页面会给出带 token 的下载与配置命令，形如：

```powershell
mkdir C:\actions-runner; cd C:\actions-runner
# 按页面给的 URL 下载 actions-runner-win-x64-*.zip 并解压到当前目录
.\config.cmd --url https://github.com/<owner>/<repo> --token <每次页面新生成的 token> `
             --name r3e-gpu --labels self-hosted,windows,gpu --work _work
```

要点：

- `--labels` 里 **`self-hosted` / `windows` 是自动带的，`gpu` 必须自己加**；`ci.yml` 里
  `runs-on: [self-hosted, windows, gpu]` 三个标签全命中才会派活。
- `--name` 起得有意义（`r3e-gpu`），以后换机器时 artifact / 日志里能一眼认出。

### 4.1 关键坑：不要 `install service`

`.\svc.cmd install` 装成 Windows 服务后，进程跑在 **Session 0**，那里没有桌面、没有窗口站，
`r3e` 建窗口必然失败 → 退出码 `2` → 严格模式直接红，且原因看起来像"没显卡"，极难排查。

正确做法是让 runner 跑在**已登录用户的交互式会话**里，二选一：

1. **手动前台**：登录桌面后执行 `.\run.cmd`，窗口挂着别关（适合一台专用机器）；
2. **开机自动**（推荐）：配自动登录该用户 + 任务计划程序，触发器选"用户登录时"，
   动作 `C:\actions-runner\run.cmd`，"不管用户是否登录都运行"**不要勾**。

判断标准只有一条：这台机器上手动跑 `cargo test -p r3e-app --test visual_regression`（带
`R3E_VISUAL_REGRESSION=1`）能通过，才说明会话是对的。

### 4.2 机器习惯

- **别让它睡眠**：电源计划设"从不睡眠"，屏保无所谓，睡眠会让 job 卡在 queued/超时。
- **本地缓存就行**：`visual-gpu` 刻意不挂 `Swatinem/rust-cache`——自建机器的 `target/` 本来就在本地，
  再叠一层远端缓存只会增加不确定性和上传时间。
- **定期清产物**：`crates/r3e-app/target/visual-regression/`、`_work/` 会缓慢长大，脏了手动删即可；
  `cargo clean` 不必须。
- **这台机器是共享资源**：`timeout-minutes: 30` 是防它被别的活占着时把 PR 卡满 6 小时的兜底。

## 5. 打开开关

仓库 `Settings` → `Secrets and variables` → `Actions` → `Variables` → `New repository variable`：

```
Name : R3E_GPU_RUNNER
Value: true
```

`ci.yml` 里是 `if: vars.R3E_GPU_RUNNER == 'true'`。变量没建时 job 直接 skipped，任何人 fork / 无 GPU 环境
都不会因为这道门变成满屏红。

## 6. 验它真的在拦（别只信绿灯）

配置完至少做一次"故意变红"的验证，否则你只是又加了一个永远绿的 job：

1. 随便改一处画面（例如把某材质基色改 0.02），提交到分支、开 PR；
2. `visual-gpu` 的**两行**应变红，`assert_png_matches` 的消息里带最坏点与坏点比例，
   artifact `visual-gpu-vulkan-artifacts` / `visual-gpu-dx12-artifacts` 里各有实际图与差异图
   （按后端分名，否则下载后分不清是哪条后端拍的）；
3. 确认红了之后**回滚那处改动**，再确认它变绿。

第二条对照实验：在自建 runner 上临时把 `R3E_VISUAL_STRICT` 去掉（改分支里的 `ci.yml`），
把显卡驱动禁用或让窗口建不出来，应当看到"跳过 + passed"——这正好证明严格模式不是摆设。

第三条对照实验（钉后端这道）：把某一行矩阵的 `backend` 临时改成这台机器没有的后端（例如只留核显时填 `dx12`
而驱动不支持），或用 `WGPU_BACKEND=dx13` 这种错名字 —— 应当看到**进程退出码 3 + 用例红**，
而不是"悄悄回退到 vulkan 后照旧通过"。真机上正常的两行日志长这样：
`后端矩阵生效: 要求 vulkan / 实际 Vulkan`、`后端矩阵生效: 要求 dx12 / 实际 Dx12`。

> 顺带一条实测口径（决定要不要按后端分基准）：本机 dx12 实际图与 vulkan 基准的**最大通道差为 1、
> 超阈值（>2）像素 0 / 230,400**，所以现在两个后端共用一张基准图和同一套默认容差；哪天某一行真抖了，
> 只放宽那一行的 `R3E_VISUAL_CHANNEL_DELTA` / `R3E_VISUAL_BAD_RATIO`，别改基准图。

## 7. 边界

- **基线跨机器不通用**：`capture_frame_640x360.png` 是逐像素的，不同 GPU / 驱动 / 系统字体渲染都会差。
  换卡、换驱动大版本、换系统字体的机器上应该用 `R3E_UPDATE_BASELINE=1` 在本机重建后**提交新基线**，
  并且这次提交要在 PR 里明说"基线因换 GPU 而重建"。
- **自建 runner 是单点**：它离线 / 占用时 `visual-gpu` 会排队，不阻塞别的三道门。
  真到长期不可用，就把 `R3E_GPU_RUNNER` 删掉（回到 skipped），而不是让 PR 天天等。
- **矩阵里没有 gl，是刻意排除而非遗漏**：本工程的 SSAO 管线在 GL 后端会被 `wgpu` 直接判死
  （`textureLoad` from depth textures 不被 GLSL 支持，`create_render_pipeline` 校验失败）。
  那是后端能力缺失，不是"两个后端画出不同像素"，硬加进来只会得到一条永远红、没人看的 job。
- **不是安全边界**：PR 来自 fork 时，代码会在你的机器上跑。仓库是私有的前提下才可接受；
  公开仓库请改用 `pull_request_target` 之外的隔离方案，别把带 GPU 的机器暴露给任意 PR。
