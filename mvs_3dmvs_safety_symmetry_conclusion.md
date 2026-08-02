# MVS 与 3DMVS Rust 包装的安全性、优雅性与对称性结论

## 1. 结论摘要

当前没有证据表明 `mvs-sdk-rs` 因为使用 `unsafe impl Send for Camera` 就是不健全的包装。

`unsafe impl Send` 的含义不是“这个类型不安全”，而是编译器无法根据字段自动推导 `Send`，需要维护者人工证明：把该值整体移动到另一线程不会破坏 Rust 的内存安全和原生 SDK 的线程约束。只要证明成立，公开 API 仍然可以是健全的安全抽象。

现有 MVS 包装具备支持这一结论的基础：

- Camera 独占原生设备 handle；
- 官方 MVS 示例展示了设备 handle 的跨线程使用；
- Camera 通过 `PhantomData<Cell<()>>` 明确保留 `!Sync`；
- Camera 持有 `Arc<Sdk>`，不会因为移动 Camera 而提前释放 SDK token；
- callback closure 具有 `Send` 约束，相关共享状态使用了同步原语。

因此，对现有实现更准确的评价是：

> `Camera: Send + !Sync` 的方向合理，当前没有发现足以直接判定其不健全的问题；但手工 `Send` 的覆盖范围较大，回调退出屏障和析构失败路径仍值得进一步审计和加固。

对于 `3dmvs-sdk-rs`，不能直接复制同一个 `unsafe impl Send for Camera`。3DMVS Camera 借用包含共享状态的 Runtime，而 Runtime 当前包含未同步的 `Cell`、没有线程约束的 Driver trait object，以及独立于 Gate 更新的 handle 计数。直接绕过这些字段会产生真实的 Rust 数据竞争风险。

两套包装应当追求相同的公开安全语义，但内部实现必须分别服从各自 SDK 的初始化、终结、回调和异步操作契约。

## 2. 应当对称的公开语义

建议两套包装统一采用以下能力边界：

| 类型或行为 | 推荐线程语义 | 说明 |
|---|---|---|
| `Camera` | `Send + !Sync` | 可以转移唯一所有权，但不能无同步共享同一实例 |
| 拉流或回调采集 session | `Send + !Sync` | 移动 session 等价于转移独占操作权 |
| SDK 原生 buffer guard | `!Send + !Sync` | 原生缓冲区的释放线程和有效窗口需要保守处理 |
| `OwnedFrame` | `Send + Sync` | 数据已经复制，不再依赖 SDK 内存 |
| callback closure | 至少 `Send + 'static` | callback 可能由 SDK 工作线程调用 |
| 显式关闭 | 返回可观察错误 | 调用者可以处理 stop、close、destroy 失败 |
| `Drop` | best-effort、不得 panic | Drop 负责兜底，不隐瞒显式关闭接口的重要错误 |

`Send` 只表示所有权可以跨线程转移，不表示同一句柄可以接受任意并发调用。`!Sync` 应由类型系统明确表达，而不是只依赖文档约定。

## 3. 不应强行对称的部分

MVS 和 3DMVS 虽然来自同一厂商，但不能据此假定以下契约完全一致：

- `Initialize` 与 `Finalize` 是否必须在同一线程配对；
- SDK 是否允许重复初始化或初始化失败后重试；
- `Finalize` 是否允许发生在最后一个共享引用被任意线程释放时；
- 注销或销毁 handle 返回时，是否已经等待全部 callback 退出；
- 异步文件传输是否可以在不同线程启动、轮询和关闭；
- 不同设备 handle 是否可以真正并发调用。

因此，应该追求“公开安全模型对称”，而不是为了代码外观一致而强行统一生命周期实现。

特别是 `Sdk`：

- 如果 MVS 包装有意让 native SDK 存活到进程结束，那么使用进程级单例且不调用 `Finalize` 可以是合理策略；
- 如果 3DMVS 需要确定性 `Finalize`，则应保留 owner token，并明确所有 Camera 关闭后才能终结；
- 在没有厂商证据前，不应仅为对称性让两者都变成 `Arc<Sdk>`，也不应让最后一个 Arc 在任意线程隐式调用 `Finalize`。

## 4. 对 mvs-sdk-rs 的优先改进建议

### 4.1 将 `unsafe impl Send` 下沉到最小句柄边界

当前由整个公开 Camera 手工实现 `Send`。更易审计的结构是为 opaque handle 建立私有 newtype：

```rust
use std::cell::Cell;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::NonNull;

struct DeviceHandle(NonNull<c_void>);

// SAFETY: 厂商的线程模型允许设备 handle 的唯一所有权跨线程转移。
// Rust 不会解引用该指针，所有原生调用仍由 Camera 独占控制。
unsafe impl Send for DeviceHandle {}

// 不实现 Sync。

struct Camera {
    handle: Option<DeviceHandle>,
    // 其他字段必须各自满足 Send，编译器才会自动推导 Camera: Send。
    _not_sync: PhantomData<Cell<()>>,
}
```

完成后应删除对整个 Camera 的宽泛 `unsafe impl Send`，让 Camera 的 `Send` 尽可能由字段自动推导。

这样做的收益是：未来若 Camera 新增 `Rc`、线程绑定 guard 或其他非 `Send` 字段，编译器会立即拒绝 `Camera: Send`，而不会被旧的手工实现掩盖。

### 4.2 强化 callback 注销、drain 和迟到调用处理

需要确认厂商是否书面保证：

1. callback 注销或 handle 销毁返回后，不会再使用旧的 `pUser`；
2. 返回时所有正在执行的 callback 已经结束；
3. 注销或销毁失败时，callback 注册和 `pUser` 的保留状态是什么。

如果缺少这些保证，建议采用更强的 cookie registry 方案：

1. 原生 `pUser` 只保存不可解引用且永不复用的整数 cookie；
2. 全局同步 registry 持有真正的 callback entry；
3. trampoline 先通过 cookie 查找并登记 in-flight，再读取 payload；
4. 注销时先拒绝新准入，再移除 cookie；
5. 等待 in-flight 归零后释放 closure；
6. 迟到 callback 因查不到 cookie 而在读取 payload 前返回。

推荐的退出顺序为：

> revoke admission → remove cookie → drain in-flight callbacks → stop acquisition → close device → destroy handle

如果销毁失败且无法证明原生端已经释放 `pUser`，泄漏相关 token 比释放后形成 use-after-free 更安全。

### 4.3 增加显式、可观察的关闭接口

建议提供：

```rust
pub fn close(self) -> Result<(), CleanupError>;
```

显式 close 应报告 stop、callback unregister、close device 和 destroy handle 的错误。`Drop` 继续执行相同顺序的 best-effort 清理，但不得 panic，也不能成为观察清理错误的唯一途径。

handle 建议保存为 `Option<DeviceHandle>`，清理开始时先 `take()`，从结构上保证只消费一次。

### 4.4 使用明确的 Camera 状态机

仅用 `grabbing: bool` 很难表达原生调用部分成功后的不确定状态。建议至少区分：

- `Open`；
- `Grabbing`；
- `Faulted`；
- `Closed`。

如果 `StartGrabbing` 返回失败，而厂商没有保证该调用完全没有生效，应进入 `Faulted`。随后只允许保守清理，并在 Drop 中尝试 stop、close 和 destroy。

类似地，callback 注册、替换、注销失败后，也应记录足够状态，确保旧 token 不会被过早释放。

### 4.5 引入可测试的 Driver/FFI 边界

建议把直接 sys 调用隔离到私有 Driver 或函数表层，使测试可以注入每一个 FFI 失败点。至少应覆盖：

- create 成功但 open 失败；
- start 返回失败但原生状态可能部分改变；
- stop、close、destroy 分别失败；
- callback 注册失败但原生端可能已经保存 `pUser`；
- callback 正在执行时注销或 Drop；
- 注销后的迟到 callback；
- Camera 移动到另一线程后显式 close 和隐式 Drop；
- callback closure panic 和 panic payload 的析构异常。

编译期 trait 断言只能证明类型表面约束；上述 mock/failpoint 测试才真正覆盖清理和并发生命周期。

### 4.6 明确 Sdk 的真实生命周期语义

当前若 native SDK 只初始化一次、包装不调用 `Finalize`，那么每次初始化都构造一个新的 `Arc<Sdk>` 容易让使用者误以为 Arc 在管理 native 生命周期。

应明确选择以下一种设计：

#### 方案 A：进程生命周期单例

- 使用 `OnceLock<Sdk>` 或 `OnceLock<Arc<Sdk>>` 保存唯一实例；
- `init` 返回 `&'static Sdk` 或同一个 Arc 的 clone；
- 文档明确声明 native SDK 存活到进程结束；
- Camera 不依赖一个仅具装饰作用的独立 Arc token。

#### 方案 B：确定性 owner 与 Finalize

- `SdkOwner` 负责 Initialize/Finalize，并保持明确的线程约束；
- Camera 只能在 owner 仍有效时存在，或由同步的共享内核记录 live handle；
- shutdown 必须在所有 Camera 清理完成后执行；
- 不允许最后一个普通 Arc 在任意线程隐式 Finalize，除非厂商明确支持。

在没有完成 MVS 初始化和终结线程契约调查前，方案 A 更保守；不应为了表面上与 3DMVS 一致而仓促增加 Finalize。

## 5. 可选的 API 对称性改进

以下改动不是当前健全性的必要条件，但可以让两个仓库更一致、更容易正确使用。

### 5.1 为 MVS 引入采集 session guard

可以让 `start_grabbing` 返回独占借用 Camera 的 `Measurement`：

```rust
pub fn start(&mut self) -> Result<Measurement<'_>>;
```

session 的 Drop 负责 best-effort stop，显式 `stop(self)` 返回错误。这样能够在类型层面阻止采集期间执行不兼容操作，并与 3DMVS 的 session 模型一致。

### 5.2 原生控制操作优先使用 `&mut self`

除纯 Rust 查询外，参数读写、事件开关和其他触达同一 native handle 的操作可统一使用 `&mut self`，明确表达串行独占访问。

Camera 的 `!Sync` 已经阻止无同步的跨线程共享，因此这主要是语义强化和未来防回归措施，而不是对当前已确认漏洞的修复。

### 5.3 为 raw handle 暴露建立清晰边界

如果继续公开 raw handle，应清楚记录：

- 返回值不转移所有权；
- 调用者不得自行 close 或 destroy；
- handle 只在 Camera 存活且未关闭时有效；
- 通过 raw handle 调用原生接口可能破坏安全包装的状态机，相关责任属于调用者的 unsafe 代码。

也可以提供带生命周期的借用句柄 newtype，或仅在低层 feature 中暴露 escape hatch。

## 6. 推荐实施顺序

建议按以下顺序改造 `mvs-sdk-rs`：

1. 引入 `DeviceHandle` newtype，将 `unsafe impl Send` 下沉到该叶节点；
2. 保持 Camera 明确 `!Sync`，并让其 `Send` 由字段自动推导；
3. 审计厂商 callback quiescence 契约，必要时引入 cookie registry 和 drain；
4. 增加显式 `close(self)`、清理错误类型和故障状态机；
5. 引入可替换 Driver，补齐失败注入和跨线程 Drop 测试；
6. 明确进程级 Sdk 与 `Finalize` 策略；
7. 最后再评估采集 session、`&mut self` 接收者和 raw handle API 等破坏性改进。

每一步都应增加编译期断言：

```rust
assert_impl_all!(Camera: Send);
assert_not_impl_any!(Camera: Sync);
assert_not_impl_any!(FrameGuard<'static>: Send, Sync);
assert_impl_all!(OwnedFrame: Send, Sync);
```

同时应确保删除宽泛的 Camera `unsafe impl Send` 后，上述断言仍然通过。这说明所有内部字段都已经真实满足所声明的线程契约。

## 7. 最终判断

MVS 与 3DMVS 应当在用户可观察的安全模型上对称：设备所有权可以移动、同一设备不能无同步共享、原生借用缓冲区受线程和生命周期限制、复制后的数据可以自由传递、清理顺序确定且错误可观察。

但两套 SDK 的初始化、Finalize、callback quiescence 和异步操作契约必须分别调查。来自同一厂商只能提高“行为可能相似”的可信度，不能代替原生契约证据。

最终推荐可以概括为：

> 不把现有 `mvs-sdk-rs` 定性为不安全；将其视为一个方向正确但 unsafe 边界偏宽、生命周期证明仍可加固的实现。通过下沉 handle 的 `Send`、强化 callback drain、显式化清理错误和增加可测试 FFI 状态机，可以在保持 `Camera: Send + !Sync` 的同时，使包装更优雅、更容易持续审计，并与 3DMVS 形成真正的安全语义对称。
