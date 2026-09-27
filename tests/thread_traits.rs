//! 公开类型的线程约定。

use mvs_sdk::{Camera, DeviceInfo, FrameGuard, Sdk};

macro_rules! assert_not_impl {
    ($type:ty: $bound:path) => {
        const _: fn() = || {
            trait AmbiguousIfImplemented<Marker> {
                fn marker() {}
            }
            impl<T: ?Sized> AmbiguousIfImplemented<()> for T {}
            struct ImplementsBound;
            impl<T: ?Sized + $bound> AmbiguousIfImplemented<ImplementsBound> for T {}
            let _ = <$type as AmbiguousIfImplemented<_>>::marker;
        };
    };
}

// 同一 handle 的调用必须由 owner 串行发起；buffer 不能离开取流线程。
assert_not_impl!(Camera: Sync);
assert_not_impl!(FrameGuard<'static>: Send);

// 会话与设备信息可以共享，相机可以移动到工作线程。
#[test]
fn public_types_follow_the_thread_contract() {
    fn assert_send<T: Send>() {}
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<Sdk>();
    assert_send_sync::<DeviceInfo>();
    assert_send::<Camera>();
}
