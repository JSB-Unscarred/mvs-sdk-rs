# Camera 显式关闭与 Drop 兜底

```mermaid
sequenceDiagram
    autonumber
    actor Caller as 调用方
    participant Camera as Camera
    participant Runtime as Arc RuntimeCore
    participant Slots as Box callback slots
    participant Native as MVS SDK

    Caller->>Camera: close()
    alt 当前线程位于任一 MVS callback
        Camera->>Slots: 静默 closure 并封存 Box slots
        Camera->>Camera: 消费 handle owner，保留 live 计数
        Camera-->>Caller: CleanupError(MvsError::InvalidState(..))
        Camera->>Runtime: 释放 session lease
        Note over Camera,Native: 不调用 native teardown；orphan handle 阻止 Finalize
    else owner 线程
        Camera->>Slots: 清除 stored closure
        opt 正在取流
            Camera->>Native: MV_CC_StopGrabbing()
        end
        opt callback 已注册
            Camera->>Native: 逐项注册 NULL callback
        end
        Camera->>Native: MV_CC_CloseDevice()
        Camera->>Native: MV_CC_DestroyHandle()
        alt DestroyHandle 成功
            Camera->>Slots: 释放 Box slots
            Camera->>Runtime: 释放 session lease
            Camera-->>Caller: Ok 或 CleanupError(首个失败操作与错误, destroyed=true)
        else DestroyHandle 失败
            Camera->>Slots: 遗留空 Box slots，防止 pUser 悬垂
            Camera->>Runtime: 释放 session lease；live 计数继续生效
            Camera-->>Caller: CleanupError(DestroyHandle 错误, destroyed=false)
        end
    end
```

单步失败不打断后续清理，`DestroyHandle` 始终最后执行。显式 `close(self)` 会消费 Camera
并只尝试一次；`CleanupError` 分别保留 Destroy 前首个失败操作与错误、独立的
Destroy 错误和 handle 是否已确认销毁。返回错误后不能用同一 Camera 重试，结果只用于
诊断和宿主策略。Destroy 失败时保留 live handle 状态与空 callback slot，`Sdk::shutdown`
因 live handle 返回 `MvsError::NativeHandlesLive`，不调用 Finalize。

`Drop` 在普通 owner 线程走相同 teardown 并忽略错误；若当前线程位于任一 MVS callback，
则静默 closure、封存 native 可能引用的 slot，并消费本地 handle owner，不重入 native
生命周期接口。live handle 继续阻止 Finalize。wrapper 不增加 callback drain；普通 owner
teardown 依赖 SDK 的 Stop、Close、Destroy 同步约定，closure `Arc` 只保活已经进入 Rust
的调用，RuntimeCore `Arc` 只表达 session lease。
