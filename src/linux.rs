use std::net::IpAddr;

use anyhow::Error;
use libc::{RTMGRP_IPV4_IFADDR, RTMGRP_IPV6_IFADDR, RTMGRP_LINK};
use neli::{
    consts::{nl::NlTypeWrapper, socket::NlFamily},
    genl::Genlmsghdr,
    socket::NlSocketHandle,
};

pub fn on_change<F: Fn() -> (Option<IpAddr>, Option<IpAddr>)>(f: F) -> Result<(), Error> {
    f();

    let mut sock = NlSocketHandle::connect(
        NlFamily::Route,
        None,
        &[
            RTMGRP_LINK as u32,
            RTMGRP_IPV4_IFADDR as u32,
            RTMGRP_IPV6_IFADDR as u32,
        ],
    )
    .or_else(|_| {
        NlSocketHandle::connect(
            NlFamily::Route,
            None,
            &[RTMGRP_LINK as u32, RTMGRP_IPV4_IFADDR as u32],
        )
    })?;
    for msg in sock.iter::<NlTypeWrapper, Genlmsghdr<u8, u16>>(true) {
        let _msg = msg?;
        f();
    }

    Ok(())
}
