# Sdk shutdown 的终态约束

`Sdk` 是进程内唯一 owner。`DeviceList<'sdk>`、`DeviceInfo<'sdk>` 与 `Camera<'sdk>`
借用它，因此 Rust 会在编译期阻止 native 资源存活时消费 `Sdk`。

```mermaid
sequenceDiagram
    autonumber
    actor App as 应用
    participant State as 进程 atomic state
    participant Sdk as Sdk owner
    participant Native as MVS SDK

    App->>State: Sdk::init()
    alt Unused
        State->>State: Unused → Active
        Sdk->>Native: MV_CC_Initialize()
        alt Initialize 成功
            Native-->>Sdk: Sdk owner
        else Initialize 失败
            State->>State: Active → Terminated
            Native-->>App: native MvsError
        end
    else Active
        State-->>App: MvsError::SdkInUse
    else Terminated
        State-->>App: MvsError::SdkTerminated
    end

    App->>Sdk: shutdown(self)
    Note over App,Sdk: live borrow 存在时本调用无法编译
    State->>State: Active → Terminated
    Sdk->>Native: MV_CC_Finalize()
    Native-->>App: Ok 或 native MvsError
```

官方 CHM 限定单进程仅执行一次 Initialize 与 Finalize。Initialize 失败或 Finalize 尝试后
均进入终态，后续 `Sdk::init` 返回 `SdkTerminated`；失败路径不重试 native 调用。
