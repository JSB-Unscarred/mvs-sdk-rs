# Camera 显式关闭与 Drop 兜底

```mermaid
sequenceDiagram
    autonumber
    actor Caller as 调用方
    participant Camera as Camera
    participant Slots as Callback slots
    participant Native as MVS SDK

    Caller->>Camera: close() / Drop
    alt owner 线程
        Camera->>Slots: stop_accepting()
        opt 正在取流
            Camera->>Native: MV_CC_StopGrabbing()
        end
        opt callback 已注册
            Camera->>Native: 逐项注册 NULL callback
        end
        Camera->>Slots: 等待在途 closure 并释放 closure
        Camera->>Native: MV_CC_CloseDevice()
        Camera->>Native: MV_CC_DestroyHandle()
        Camera-->>Caller: 首个错误或 Ok
    else Camera 自身 callback
        Camera->>Slots: stop_accepting()
        Camera-->>Caller: CallOrder，并保留 SDK in-use 计数
    end
```

owner 线程上的单步失败不会中断后续清理，`DestroyHandle` 是最后一步。显式 `close` 返回
首个错误；`Drop` 执行同一路径并忽略结果。若 callback 间接析构自己的 Camera，wrapper
会停用 slot、保留 native backing 与 SDK in-use 计数；标准用法由 owner 线程关闭 Camera。
