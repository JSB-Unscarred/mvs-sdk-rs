# MVS Callback 生命周期改造：最小实施计划

## 1. 决策与范围

本批只补两个尚未闭环的问题：

1. callback 的 Rust payload、native 借用数据与 Camera teardown 的生命周期；
2. image callback 与主动取帧不能混用。

现有的注销 API、`Faulted` 状态、消费式 `close`、`CleanupError`、一次性 handle
消费和 teardown 测试缝全部复用，不重新设计。

### 唯一新增的 native 生命周期契约

本实现接受以下常规 FFI 契约：

> `MV_CC_DestroyHandle(handle)` 返回 `MV_OK` 后，不会再启动 callback，也不存在仍会访问
> callback、`pUser` 或 event-name backing 的已启动/已调度 callback。

callback 的 `data` / `info` 仍只在本次 trampoline 返回前有效；SDK 必须原样回传注册时的
`pUser`。这些属于 callback API 本身的基本前提。

不假定 unregister、`StopGrabbing` 或 `CloseDevice` 会 drain callback，也不假定
register/unregister 失败前完全没有保存参数。因此：

- Slot 和 event name 一直保留到 `DestroyHandle(MV_OK)`；
- `DestroyHandle` 失败时，泄漏 handle、Slot 和 event-name backing；
- 不防御成功 destroy 之后仍继续调用 callback 的 SDK 违约行为。

接受这一边界后，不再引入进程级 registry、整数 cookie、camera-wide gate、Condvar、
in-flight 计数或 retired callback 队列。

## 2. 最小内部模型

```rust
struct CallbackSlot<C> {
    accepting: AtomicBool,
    callback: Mutex<Option<C>>,
}

struct CallbackRecord<C> {
    slot: Box<CallbackSlot<C>>,
    native_registered: bool,
}

struct EventRecord {
    name: CString,
    callback: CallbackRecord<EventCallback>,
}
```

- image、exception 各至多一个 `CallbackRecord`；每个不同 event name 一个
  `EventRecord`。
- `pUser` 指向 `Box<CallbackSlot<_>>` 的稳定堆地址。Camera 或 event 容器移动不会改变该
  地址。
- record 首次创建后保留到 Camera destroy；同名 event 永远复用原有 Slot 和 `CString`。
- `native_registered` 只表示 Rust 已确认 native 注册成功。注册失败的不确定状态继续使用
  现有 `UncertainRegistration`；失败立即进入现有 `Faulted`，不提供恢复流程。
- callback closure 只在 Slot 内；不再为每次替换分配新的 native token。

同一个 Slot 注销后可以重新注册。方案不追踪 native registration generation：重新注册后，
任何随后通过该 Slot 准入的 callback 都调用当前 closure，即使该 native 调用可能源自更早的
注册周期。这是删除 cookie/generation 后明确接受的语义。

## 3. Trampoline

三类 trampoline 使用同一协议：

1. 检查 `pUser`，并用 thread-local Slot 地址栈拒绝同一 Slot 的同步重入；
2. 压入当前 Slot 地址，建立 callback-context guard；
3. 快速检查 `accepting`，然后锁 `callback` mutex；
4. 在锁内再次检查 `accepting` 和 `Option::is_some()`；
5. 只有准入成功后才解引用 `data` / `info`、构造 `Frame<'_>` / `EventInfo<'_>` 并调用
   closure；
6. mutex guard 覆盖 native 借用和 closure 调用；callback-context guard 继续覆盖 panic
   payload 处理和 callback 自清理；
7. 若退出时 `accepting == false`，trampoline 在锁内 `take()` 当前 closure，并在锁外隔离
   其析构 panic。

同一 Slot 重入直接返回，避免 `FnMut` mutex 自锁；不同 Slot 仍可嵌套。TLS 只保存 Slot
地址，不引入 Camera ID、cookie 或通用重入状态机。

closure panic、panic payload Drop panic、cleanup/extern 路径上的 closure capture Drop panic
继续由现有的 unwind containment helper 隔离。wrapper 自身调用 native
register/unregister/cleanup 时不得持有 Slot mutex；用户 closure 为保持 `FnMut` 串行语义，
仍在持有该 mutex 时执行。

## 4. 注册、替换与注销

同一 Camera 的 callback context 中，`start_grabbing`、`stop_grabbing` 以及三类 callback 的
注册、替换和注销统一返回 `MvsError::CallOrder`。这既避免当前 Slot 自锁，也避免在
`Frame<'_>` / `EventInfo<'_>` 仍借用 native 数据时改变采集生命周期。

### 4.1 首次注册或注销后重新注册

1. 先完成 event name 校验、Slot 分配和容器插入；
2. 在 Slot 中设置 `callback = Some(f)`、`accepting = true`；
3. 不持有 Slot mutex 调用 native register，使同步 callback 可以正常进入；
4. 成功后设置 `native_registered = true`；
5. 失败后设置 `accepting = false`，取出 closure，保留 Slot/name，记录现有
   `UncertainRegistration` 并进入 `Faulted`。

失败后即使 native 部分保存了 `pUser`，迟到 callback 也只会看到 disabled/`None` Slot。

### 4.2 替换已注册 callback

若 `native_registered == true`，只在 Slot mutex 内替换 closure，不再次调用 native
register。正在执行的 callback 与替换由同一 mutex 线性化；旧 closure 在锁外安全析构。

### 4.3 注销

1. 若当前线程正在执行本 Camera 任一 callback，返回 `MvsError::CallOrder`；
2. 设置 `accepting = false`；
3. 锁 Slot 并 `take()` closure。该锁会等待正在执行的 closure 完成；
4. 在锁外安全析构 closure，并在锁外调用 native unregister；
5. 成功后设置 `native_registered = false`；失败时保留保守注册状态并进入 `Faulted`；
6. 已 inactive 的重复 unregister 返回 `Ok(())`。

普通 unregister 返回后 Rust closure 已静默，但 Slot 和 event name 仍保留到 destroy。

image callback 的首次注册、替换和注销只允许 Camera 处于 Open；grabbing 期间一律返回
`CallOrder`。exception/event callback 不参与采集模式选择。

## 5. Close 与 Drop

### 5.1 普通线程

显式 close 和 Drop 复用现有 cleanup 骨架：

1. 将全部 Slot 的 `accepting` 设为 false；
2. 逐个锁 Slot、`take()` closure，不同时持有多个 Slot 锁；这会 drain 已进入用户 closure
   的 callback；
3. 在锁外安全析构 closures；
4. 沿用现有 best-effort 顺序：stop → unregister 已确认或不确定的 callback → close device
   → destroy handle；
5. 任一步失败仍继续后续步骤，并沿用现有 `CleanupError<Vec<CleanupFailure>>`；
6. destroy 成功后释放空 Slot 和 event names；destroy 失败则泄漏 handle、Slot 和 event-name
   backing。

普通 Drop 允许等待 Slot mutex。当前 Drop 本就会执行可能阻塞的 native cleanup，不再为了
non-blocking 策略在正常 in-flight callback 上泄漏 Camera。

### 5.2 Camera 自身 callback context

Camera 通过检查自己的 Slot 地址是否出现在 TLS 栈中识别该路径：

- `close` / Drop 不等待当前 Slot，也不调用 stop/unregister/close/destroy；
- 对全部 Slot 设置 `accepting = false`，能 `try_lock` 的 Slot 立即取出 closure；正在执行的
  trampoline 在退出时自行取出 closure；
- 定向泄漏 handle、空 Slot 和 event-name backing；
- 显式 close 返回一个明确的 callback-context cleanup error，Drop 静默记录后返回。

这条特殊路径只解决自等待和 callback 内 teardown，不引入后台 reaper 或延迟清理线程。

## 6. 采集模式

直接扩展现有状态，不新增平行 `AcquisitionState`：

```rust
enum AcquisitionMode {
    Callback,
    Polling,
}

enum CameraState {
    Open,
    Grabbing(AcquisitionMode),
    Faulted,
    Closed,
}
```

- `start_grabbing` 只允许从 `Open` 进入：image record 已确认 active 时选择 `Callback`，否则
  选择 `Polling`；在调用 native start 前先写入该状态，使同步 callback 也观察到正确模式；
- native start 失败进入现有 `Faulted`；
- `get_image_buffer` 只允许 `Grabbing(Polling)`；
- image callback 注册、替换和注销只允许 `Open`；
- `stop_grabbing` 接受任意 `Grabbing(_)`，成功回到 `Open`，失败进入 `Faulted`；
- exception/event callback 不影响 acquisition mode。

## Commit 1 — `fix: keep callbacks in stable per-camera slots`

### 修改

- 用稳定 `CallbackSlot`/`CallbackRecord` 一次迁移 image、exception、event trampoline；
- 删除 `Arc::into_raw` token、`clone_callback` strong-count 操作和全部 retired callback 队列；
- active callback 的替换改为纯 Rust Slot 替换；
- 失败继续复用现有 `Faulted + UncertainRegistration`；
- event records 按不同 name 长期存在并复用 backing；
- 增加最小 TLS Slot 栈及 callback-context close/Drop 分支；
- 修改现有 cleanup，使 Slot 只在 destroy 成功后释放；
- 复用并扩展现有窄函数表测试缝，不增加 Driver trait 或新的生产抽象；
- 在 callback 模块的 SAFETY 注释中记录本计划采用的 destroy 契约。

### 验收

- 同类/同名 callback 多次替换时 `pUser` 地址不变，native register 不重复调用；
- 注册期间同步 callback 可以执行；注册失败后的迟到 callback 不读取 native 数据、不调用
  closure；
- unregister 会等待当前 closure，返回后迟到 callback 静默；
- 注销后重新注册使用当前 closure，并符合本计划声明的无 generation 语义；
- 同一 Slot 同步重入不死锁；
- 连续替换不会增长 Slot 或 closure capture 数量；
- callback、panic payload 和受控 closure 析构均不跨越 extern/Drop unwind；
- 普通 close/Drop drain callback；callback-context close/Drop 不调用 native teardown；
- destroy 成功释放 Slots，失败保留 native-dependent backing；
- 现有 cleanup 顺序、failpoint 与错误聚合测试继续通过。

## Commit 2 — `fix: keep callback and polling modes exclusive`

### 修改与验收

- 将现有 `CameraState::Grabbing` 改为携带 `AcquisitionMode`；
- `get_image_buffer` 只接受 Polling；
- grabbing 期间拒绝 image callback 注册、替换和注销；
- start/stop 失败继续进入现有 `Faulted`，不增加恢复状态；
- 同步更新 unsupported backend、README、Debug 和 API tests；
- `cargo test --workspace` 与 `cargo check --workspace --all-targets` 通过。

## 7. 完成标准

- `pUser` 在 handle 生命周期内只指向地址稳定的 Slot；
- native 数据只在 active closure 持有 Slot mutex 时借用；
- unregister/replace/close 不再需要 retired callback；
- callback 替换次数不会造成 payload 无界增长；
- 只有 `DestroyHandle(MV_OK)` 允许释放 Slot 和 event-name backing；
- callback-context teardown 不会自等待或释放当前 callback 正在借用的 native 数据；
- callback 与 polling 无法通过安全 API 混用。

其余线程模型、typestate、通用 Driver、SDK 初始化/终结和跨平台改造不在本批范围内。
