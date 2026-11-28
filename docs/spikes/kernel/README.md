# T4 fingerprint-chromium 148 macOS ARM64 能力 spike

## 结论

本 spike 在 macOS arm64 宿主上验证了 pinned `fingerprint-chromium 148.0.7778.215` DMG（资产 SHA-256 与 manifest 一致）。探针证实一组画像开关会改变 CDP 页面读回；同时发现 `--disable-spoofing=clientrects`、`font`、`gpu` 在本探针读回上没有形成可归因的独立变化，不能把“参数被接受”写成“能力已生效”。`window-size` 只改变 `window.outerWidth/outerHeight`，不改变 host-bound `screen.*`。

**重要边界：** 本机为 Apple Silicon，但 Chromium 的 WebGL 在本次有头/无头探针中返回 ANGLE/Metal 或 ANGLE/Intel 模拟设备；这不是产品默认图形栈保证。探针启动为了可重复性显式使用 `--no-sandbox`，因此结果不能证明 manifest 声明的 macOS Seatbelt 在本机已生效。一次有头启动会创建可见窗口，属预期；每次用完均终止并确认无残留内核进程。

## 资产身份

| 项目 | 实测值 |
| --- | --- |
| manifest | `docs/contracts/manifests/fingerprint-chromium-148.0.7778.215-macos-arm64.json` |
| version | `Chromium 148.0.7778.215` |
| platform | `macos-arm64` |
| DMG asset SHA-256 | `b72f091e2e1a7583eed389c4b8e3534ed355e568af8c8bbf8fc30a25e23ca679` |
| executable | `Chromium.app/Contents/MacOS/Chromium` |
| executable SHA-256 | `d918c691ce56648bc555f8f399c29fc83eb02224165a086491688d01707170cb` |
| cache | `fixtures/.cache/kernel/`（gitignored，不提交） |
| upstream asset | `https://github.com/adryfish/fingerprint-chromium/releases/download/148.0.7778.215/ungoogled-chromium_148.0.7778.215-1.1_macos.dmg` |

DMG 通过 `hdiutil attach -nobrowse -readonly` 挂载，复制完整 `Chromium.app` 后按最终可执行文件路径计算 binary SHA-256；报告不把 asset SHA-256 当作 binary SHA-256。

## 重复运行

从仓库根目录运行：

```sh
node docs/spikes/kernel/probe.mjs --suite smoke
node docs/spikes/kernel/probe.mjs --suite modes
node docs/spikes/kernel/probe.mjs --suite flags
node docs/spikes/kernel/probe.mjs --suite seeds
```

脚本启动本地 `127.0.0.1` HTTP 探针页，通过 `--remote-debugging-port` 连接 CDP，读取页面 JS 结果并写入 `docs/spikes/kernel/evidence/*.json`。临时 profile 在每次 case 结束时删除；`--keep-profile` 仅用于调试。脚本为本地 spike 使用 `--no-sandbox`，不得把该参数复制到产品运行时。

证据 JSON 包含启动 argv（临时 profile、CDP 端口与本机路径已脱敏）、资产/二进制摘要、UA/UA-CH、窗口与屏幕、canvas/WebGL/audio、字体、plugins、WebRTC 和进程退出信息。raw evidence 不含凭据、页面外部数据或真实站点内容。

## 能力矩阵

结论只指当前 macOS arm64 宿主、探针输入和当前 build；`生效`表示指定读回变化或精确 CDP 回读，`无效`表示开关值被传入但探针未观察到其目标差异，`未验证`表示没有足够的证据，不作推断。

| 开关/能力 | 结论 | 证据 |
| --- | --- | --- |
| `--fingerprint=1001/1002` | 生效 | `fingerprint-1002.json`：`hardwareConcurrency` 由 8 变为 10，canvas、audio、GPU 模型派生值变化；`baseline.json` 对照 |
| `--fingerprint-platform=windows` | 生效 | `platform-windows.json`：UA `Windows NT 10.0; Win64; x64`，UA-CH platform `Windows`/version `19.0.0`，`navigator.platform=Win32` |
| `--fingerprint-platform=linux` | 生效 | `platform-linux.json`：UA `X11; Linux x86_64`，UA-CH platform `Linux`/version `6.8.0`，`navigator.platform=Linux x86_64` |
| `--fingerprint-platform=macos` | 生效 | `platform-macos.json`：UA/UA-CH platform 与 macOS 画像回读，UA-CH version `15.6.1` |
| `--fingerprint-brand=Edge` | 生效 | `brand-edge.json`：UA 增加 `Edg/148.0.3967.54`；UA-CH brands 增加 `Microsoft Edge` |
| `--fingerprint-brand=Opera` | 生效 | `brand-opera.json`：UA 增加 `OPR/148.0.0.0` |
| `--lang=en-US` / `--accept-lang=en-US,en;q=0.9` | 生效 | `lang-en.json`：`navigator.language=en-US`、`navigator.languages=["en-US","en;q=0.9"]`；`lang-zh.json` 是对照 |
| `--timezone=America/New_York` | 生效 | `timezone-new-york.json`：`Intl.DateTimeFormat().resolvedOptions().timeZone=America/New_York` |
| `--fingerprint-hardware-concurrency=12` | 生效 | `hardware-12.json`：`navigator.hardwareConcurrency=12`；值属于 manifest macOS 允许集合 |
| `--disable-spoofing=canvas` | 生效（变化方向需按 kernel 解释） | `disable-canvas.json`：canvas full hash 与 baseline 不同；audio、clientRects、GPU 读回保持对照 |
| `--disable-spoofing=audio` | 生效（变化方向需按 kernel 解释） | `disable-audio.json`：audio hash 与 baseline 不同；canvas/clientRects 对照不变 |
| `--disable-spoofing=clientrects` | 无效（本探针） | `disable-clientrects.json`：rect 仅有重复运行浮点微差，尺寸与 baseline 相同；未观察到目标性变化，不得进入能力清单为“已验证生效” |
| `--disable-spoofing=font` | 无效（本探针） | `disable-font.json`：候选字体可采集性与 metrics 与 baseline 相同；不把 `document.fonts.check` 接受参数当作字体伪装能力 |
| `--disable-spoofing=gpu` | 生效（GPU 读回变化） | `disable-gpu.json`：GPU vendor/renderer 从 ANGLE Intel 变为 ANGLE Apple Metal；此变化包含宿主图形后端因素，产品需单独确认预期 |
| `--window-size=1920,1080` | 生效（窗口几何） | `window-1920x1080.json`：`outerWidth/outerHeight=1920/1080`；`screen=800x600`、DPR=1 未变 |
| `--fingerprint-brand=Chrome/Vivaldi` | 未验证 | 本次仅抽样 Edge/Opera；manifest 仍声明 Chrome/Vivaldi，不能由其他品牌推断 |
| `--fingerprint-platform-version` 独立行为 | 未验证 | 与 platform 一起传入并在 UA-CH 回读；未做仅版本参数的隔离对照 |
| `--proxy-server` | 未验证 | 本 spike 没有受控代理 fixture，不对代理路径/认证/DNS 作结论 |
| `--disable-non-proxied-udp` | 未验证 | 本 spike 未建立受控网络对照 |

**不得进入能力清单：** `clientrects`、`font` 不得仅因命令行接受或 manifest 声明就标记为本机已验证生效；`proxy-server`、`disable-non-proxied-udp`、Chrome/Vivaldi 品牌、独立 platform-version、sandbox/Seatbelt、DoH/IPv6 也未由本 spike 验证。manifest 现有 `disable-spoofing` 是候选能力声明，不能被本报告的局部探针扩大解释。

## 模式对比

| 模式 | UA | UA-CH | webdriver | outer | screen | DPR |
| --- | --- | --- | --- | --- | --- | --- |
| headful | `Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) ... Chrome/148.0.0.0 Safari/537.36` | brands `Not/A)Brand` + `Chromium 148`; platform `macOS` | `false` | `1280×720` | `2560×1440` | `2` |
| `--headless=new` | 与 headful 相同（本 build 的 headless UA patch 已移除 `HeadlessChrome`） | 与 headful 相同 | `false` | `1280×720` | `800×600` | `1` |
| 伪有头（本 spike 以 `mode=pseudo` 运行同一 headless-new 启动） | 与 headless-new 相同 | 与 headless-new 相同 | `false` | `1280×720` | `800×600` | `1` |

证据：`headful.json`、`headless-new.json`、`pseudo-headful.json`。本次伪有头 case 没有额外 UA 变换 flag；它用于验证清单 `headless_chrome_ua_patch=true` 的读回边界，不能被写成“有头等价”。`navigator.webdriver=false` 是本次 Chromium 启动方式的读回，不是反自动化保证。

`headless_chrome_ua_patch` 的结论：**只影响 UA 产品名部分**。本 build 的 `--headless=new` 已没有 `HeadlessChrome`，但 headless 其他特征仍保留：screen 从 `2560×1440` 变为 `800×600`，DPR 从 `2` 变为 `1`，且 WebGL 后端/渲染器仍是可观察宿主相关值。UA-CH 同样没有被额外“伪有头”改变；因此该字段不应被解释为消除 Headless 特征。

## Canvas / WebGL / audio

- canvas `getImageData`：完整 surface 与同坐标子区域读回均成功，无崩溃；本 probe 的 `subRegionMatchesFull=false` 是 #98 类型的子区域/完整区域一致性差异，报告归因于 kernel 已知问题，不判定 fingerprint spec 失败。
- WebGL `readPixels`：1×1、8×8 和 RGBA/UNSIGNED_SHORT 读回均成功，无崩溃；本 probe 首像素在 1×1 与 8×8 一致，记录 #99 为“本次样本未复现差异”，不能推广为问题不存在。RGBA16 调用成功，#102 的越界风险仍按 manifest 记录，单次安全返回不等于安全审计。
- audio OfflineAudioContext 读回成功；`--disable-spoofing=audio` 改变 audio hash。audio 采样未做跨平台或真实站点校准。
- 上述结果对应 `baseline.json`、`disable-canvas.json`、`disable-audio.json`、`headful.json`、`headless-new.json` 与 `pseudo-headful.json`。

## 字体、plugins、WebRTC

只记录可采集性，不判定缺陷：

- `baseline.json` 记录 10 个候选字体的 `document.fonts.check` 与 text metrics；本机候选均返回 `available=true`，metrics 是宿主相关读回。`disable-font.json` 与 baseline 未观察到目标性差异。
- `navigator.plugins` 返回 5 个内置 PDF viewer 条目（名称、描述、filename），不对其是否应被隐藏作结论。
- `RTCPeerConnection` 可创建，`iceServers=[]` 下返回 `available=true`、候选数组为空；没有外部 ICE/代理服务，因此不判定 WebRTC 泄漏或收敛。

## Seed 派生稳定性

`seed-1001-a/b/c.json` 三次独立启动的 canvas hash、audio hash、GPU vendor/renderer、deviceMemory 与 WebGL 读回一致。不同 seed 抽样：

| seed | canvas hash | deviceMemory | GPU vendor/model |
| --- | --- | ---: | --- |
| 1001 | `0171ac7af735da8468c56c2ef50979ec072a92715700851d6ac340e0599b41c8` | 32 | Intel / Iris Xe Graphics |
| 2001 | `57403d425246adb6de9896c5ca5792352f95cdfc655ee4988e753539b8705506` | 8 | Intel / UHD Graphics |
| 3001 | `bb6a642b38e091dbe3ba1830d20efe109ab42f6844fd1d022643401334b6e18f` | 16 | NVIDIA / RTX 4060 Laptop GPU |
| 4001 | `44d3ab153e4b84e9fbe51cf27f9b633436ff43f714beb2abdf40892ac61adef4` | 32 | Intel / UHD Graphics 620 |

这些是本机 build 的 seed 派生抽样，不是允许值全集。`navigator.hardwareConcurrency` 也随 seed 变化（1001=8、2001=32、3001=30、4001=28），说明未显式指定时必须使用 persona 派生结果；产品应在规格校验前明确 seed 与显式 CPU override 的优先级。

## Manifest 差异建议（不改 manifest）

本 spike 不修改 `docs/contracts/`。相对于 `fingerprint-chromium-148.0.7778.215-macos-arm64.json`，只提出建议：

1. 为 `flags` 增加一个机器可读的“验证状态/证据引用”外部报告层，而不是把当前某台 host 的结果写回 manifest；manifest 仍表达候选能力，spike evidence 表达宿主绑定实测。
2. 对 `disable-spoofing=clientrects` 与 `font` 保留候选声明，但在产品能力清单消费层不要呈现“已由本机 probe 证明生效”；当前 readback 应标为 `unverified-or-no-observed-effect`。
3. `limitations` 已正确声明 screen/DPR/font host-bound、伪有头只改 UA 以及 #98/#99 读回差异；建议后续把本次 `subRegionMatchesFull=false` 和 `WebGL 1x1/8x8 firstPixelMatches=true` 作为版本化准入报告字段，不改清单文本。
4. `headless_chrome_ua_patch=true` 与本机结果一致，但应继续保持“UA patch only”语义，不新增“headless 等同 headful”字段。
5. `seed_derived` 的 `device_memory`、`gpu_model` 与本次不同 seed 抽样一致；仍需另一台真实 Apple GPU/无 `--no-sandbox` 的准入环境后再扩大结论。

## 未验证项和原因

- 代理启动、认证、DNS、`--proxy-server`、`--disable-non-proxied-udp`：没有受控代理 fixture，避免把本地直连结果当成代理能力。
- DoH/IPv6 控制：没有 network fixture；manifest 声明不等于本机已证明。
- Chrome/Vivaldi 品牌：本次只抽样 Edge/Opera。
- platform-version 独立参数：未做隔离对照。
- Seatbelt 沙箱：脚本为启动稳定性显式使用 `--no-sandbox`；不能据此声称 Seatbelt 生效。
- 外部 WebRTC ICE 候选：没有 STUN/TURN；只记录本地空候选结果。
- RGBA16 WebGL #102：本次调用成功，但没有内存安全工具或专项越界复现，已知 issue 仍保留。
- 目标站点加载、崩溃/内存、Docker/Xvfb、签名/分发：超出 macOS arm64 CDP spike 范围。

## Evidence 文件索引

| 文件 | 内容 |
| --- | --- |
| `run.json` | 最近一次 suite 元数据 |
| `baseline.json` | 基线 UA、UA-CH、屏幕、canvas/WebGL/audio/fonts/plugins/WebRTC |
| `headful.json` / `headless-new.json` / `pseudo-headful.json` | 三种模式对比 |
| `fingerprint-1002.json` | seed 对照 |
| `platform-*.json` | Windows/Linux/macOS 画像平台 |
| `brand-edge.json` / `brand-opera.json` | 品牌与 UA-CH |
| `lang-en.json` / `lang-zh.json` | 语言和 Accept-Language |
| `timezone-new-york.json` | 时区 |
| `hardware-12.json` | CPU 核心数 |
| `disable-*.json` | spoof category 对照 |
| `window-1920x1080.json` | 窗口尺寸 vs screen |
| `seed-1001-a/b/c.json` | 同 seed 三次稳定性 |
| `seed-2001.json` / `seed-3001.json` / `seed-4001.json` | 不同 seed 差异抽样 |
