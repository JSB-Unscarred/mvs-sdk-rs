# Camera 显式关闭与 Drop 兜底

```mermaid
sequenceDiagram
    autonumber
    actor Caller as 调用方
    participant Camera as Camera
    participant Slots as Arc callback slots
    participant Native as MVS SDK

    Caller->>Camera: close()
    alt 当前线程位于任一 MVS callback
        Camera-->>Caller: CleanupError(CallOrder)
        Note over Camera,Native: 静默 callback；不调用 native teardown
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
            Camera->>Slots: 回收 native Arc token
            Camera-->>Caller: Ok 或 CleanupError(prior error, destroyed=true)
        else DestroyHandle 失败
            Camera->>Slots: 保留空 slot token，防止 pUser 悬垂
            Camera-->>Caller: CleanupError(destroy error, destroyed=false)
        end
    end
```

单步失败不打断后续清理，`DestroyHandle` 始终最后执行。显式 `close` 返回
`CleanupError`，分别保留前序首错、Destroy 错误和 handle 是否已确认销毁。Destroy
失败时保留 live handle 状态与空 callback slot，`Sdk::shutdown` 因 live handle 返回
`SdkInUse`，不调用 Finalize。

`Drop` 在普通 owner 线程走相同 teardown 并忽略错误；若当前线程位于任一 MVS callback，
只静默 callback，不重入 native 生命周期接口，live handle 继续阻止 Finalize。wrapper 不增加
callback drain；普通 owner teardown 依赖 SDK 的 Stop、Close、Destroy 同步约定，`Arc` 只保活
已经进入 Rust 的 closure。
