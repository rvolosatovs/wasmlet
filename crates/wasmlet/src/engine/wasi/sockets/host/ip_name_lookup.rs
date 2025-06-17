#![allow(unused)] // TODO: Remove

use wasmtime::component::Resource;

use crate::engine::bindings::wasi::sockets::ip_name_lookup::{
    ErrorCode, Host, HostResolveAddressStream,
};
use crate::engine::bindings::wasi::sockets::network::IpAddress;
use crate::engine::wasi::io::Pollable;
use crate::engine::wasi::sockets::{Network, ResolveAddressStream};
use crate::Ctx;

impl HostResolveAddressStream for Ctx {
    fn resolve_next_address(
        &mut self,
        self_: Resource<ResolveAddressStream>,
    ) -> wasmtime::Result<Result<Option<IpAddress>, ErrorCode>> {
        todo!()
    }

    fn subscribe(
        &mut self,
        self_: Resource<ResolveAddressStream>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        todo!()
    }

    fn drop(&mut self, rep: Resource<ResolveAddressStream>) -> wasmtime::Result<()> {
        todo!()
    }
}

impl Host for Ctx {
    fn resolve_addresses(
        &mut self,
        network: Resource<Network>,
        name: String,
    ) -> wasmtime::Result<Result<Resource<ResolveAddressStream>, ErrorCode>> {
        todo!()
    }

    //async fn resolve_addresses<U>(
    //    store: &mut Accessor<U, Self>,
    //    name: String,
    //) -> wasmtime::Result<Result<Vec<network::IpAddress>, ErrorCode>> {
    //    // `url::Host::parse` serves us two functions:
    //    // 1. validate the input is a valid domain name or IP,
    //    // 2. convert unicode domains to punycode.
    //    let host = if let Ok(host) = url::Host::parse(&name) {
    //        host
    //    } else if let Ok(addr) = Ipv6Addr::from_str(&name) {
    //        // `url::Host::parse` doesn't understand bare IPv6 addresses without [brackets]
    //        url::Host::Ipv6(addr)
    //    } else {
    //        return Ok(Err(ErrorCode::InvalidArgument));
    //    };
    //    if !store.with(|view| view.sockets().allowed_network_uses.ip_name_lookup) {
    //        return Ok(Err(ErrorCode::PermanentResolverFailure));
    //    }
    //    match host {
    //        url::Host::Ipv4(addr) => Ok(Ok(vec![network::IpAddress::Ipv4(from_ipv4_addr(addr))])),
    //        url::Host::Ipv6(addr) => Ok(Ok(vec![network::IpAddress::Ipv6(from_ipv6_addr(addr))])),
    //        url::Host::Domain(domain) => {
    //            // This is only resolving names, not ports, so force the port to be 0.
    //            if let Ok(addrs) = lookup_host((domain.as_str(), 0)).await {
    //                Ok(Ok(addrs
    //                    .map(|addr| addr.ip().to_canonical().into())
    //                    .collect()))
    //            } else {
    //                // If/when we use `getaddrinfo` directly, map the error properly.
    //                Ok(Err(ErrorCode::NameUnresolvable))
    //            }
    //        }
    //    }
    //}
}
