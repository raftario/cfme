use std::{ffi::c_void, net::IpAddr, thread};

use anyhow::Error;
use windows::Win32::{
    Foundation::HANDLE,
    NetworkManagement::IpHelper::{
        NotifyIpInterfaceChange, MIB_IPINTERFACE_ROW, MIB_NOTIFICATION_TYPE,
    },
    Networking::WinSock::AF_UNSPEC,
};

pub fn on_change<F: Fn() -> (Option<IpAddr>, Option<IpAddr>)>(f: F) -> Result<(), Error> {
    let mut handle = HANDLE::default();
    unsafe {
        NotifyIpInterfaceChange(
            AF_UNSPEC.0 as u16,
            Some(callback::<F>),
            Some((&f as *const F).cast()),
            true,
            &mut handle,
        )
    }?;

    loop {
        thread::park();
    }
}

unsafe extern "system" fn callback<F: Fn() -> (Option<IpAddr>, Option<IpAddr>)>(
    caller_context: *const c_void,
    _row: *const MIB_IPINTERFACE_ROW,
    _notification_type: MIB_NOTIFICATION_TYPE,
) {
    let f = &*caller_context.cast::<F>();
    f();
}
