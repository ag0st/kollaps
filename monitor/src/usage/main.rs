// Licensed to the Apache Software Foundation (ASF) under one or more
// contributor license agreements.  See the NOTICE file distributed with
// this work for additional information regarding copyright ownership.
// The ASF licenses this file to You under the Apache License, Version 2.0
// (the "License"); you may not use this file except in compliance with
// the License.  You may obtain a copy of the License at

//    http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![no_std]
#![no_main]

use core::mem::{self, MaybeUninit};
use memoffset::offset_of;

use redbpf_probes::socket_filter::prelude::*;
use monitor::usage::{Message, MonitorIpAddr};


//map to hold perf_events
#[map(link_section = "maps")]
static mut perf_events: PerfMap<Message> = PerfMap::with_max_entries(512);

//map to hold the bytes sent
#[map(link_section = "maps")]
static mut usage: HashMap<u32, u64> = HashMap::with_max_entries(4096);

//map to hold time that has passed
#[map(link_section = "maps")]
static mut time: HashMap<u32, u64> = HashMap::with_max_entries(4096);

program!(0xFFFFFFFE, "GPL");
#[socket_filter]
fn measure_tcp_lifetime(skb: SkBuff) -> SkBuffResult {
    let eth_len = mem::size_of::<ethhdr>();
    let eth_proto = skb.load::<__be16>(offset_of!(ethhdr, h_proto))? as u32;
    if eth_proto != ETH_P_IP {
        return Ok(SkBuffAction::Ignore);
    }

    let mut ip_hdr = unsafe { MaybeUninit::<iphdr>::zeroed().assume_init() };
    ip_hdr._bitfield_1 = __BindgenBitfieldUnit::new([skb.load::<u8>(eth_len)?]);

    if ip_hdr.version() != 4 {
        return Ok(SkBuffAction::Ignore);
    }


    // Use ttl + 32bits + 32bits for destination address for compatibility with Kernel 6.0 and above.
    // The struct iphdr has been redesigned in kernel 6.0 to contains an anonymous union instead of
    // saddr and daddr.
    // Report to the IP Header format: https://www.rfc-editor.org/rfc/rfc791#section-3.1
    // The offset bellow is calculated in term of bytes.
    // Implementation of iphdr for Kernel 5:  https://elixir.bootlin.com/linux/v5.19.17/source/include/uapi/linux/ip.h#L86
    // Implementation of iphdr for Kernel 6 : https://elixir.bootlin.com/linux/v6.0.17/source/include/uapi/linux/ip.h#L86
    let dest_addr_offset = eth_len + offset_of!(iphdr, ttl) + 4 + 4;

    //Retrieve IPs from skbuff
    let dst = MonitorIpAddr::new(
        skb.load::<__be32>(dest_addr_offset)?,
    );


    //We send new values every 25 milli seconds for a given ip, via perf_events
    unsafe {
        //retrieve size of packet
        let len: u64 = (*skb.skb).len as u64;
        let current_time = bpf_ktime_get_ns();
        match time.get(&dst.addr) {
            None => {
                time.set(&dst.addr, &current_time);
            }
            Some(old_time) => {
                match usage.get(&dst.addr) {
                    None => usage.set(&dst.addr, &len),
                    Some(value) => {
                        let new_value = (*value) + len;
                        let delta_time_ns = current_time - (*old_time);
                        if delta_time_ns > 100_000_000 { // 25 ms
                            // Calculate throughput in KBit/s
                            // Made it this way because no compatibility for floating points in eBPF
                            // * 8 is for passing to bits
                            // * 1_000_000_000 is like doing delta_time_ns / 1_000_000_000 to find second. But if we do this, no floating point and cannot express 0.0..001 seconds
                            // / delta_time_ns to find the bandwidth (size/time)
                            // * 1024 is like dividing again by 1024 to find KBit/s instead of bits/seconds
                            let throughput = (new_value * 8 * 1_000_000_000) / (delta_time_ns * 1024);
                            perf_events.insert(skb.skb as *mut __sk_buff, &Message { dst: dst.addr, throughput: throughput as u32});
                            // // reset usage and time
                            usage.set(&dst.addr, &0);
                            time.set(&dst.addr, &current_time);
                        } else {
                            usage.set(&dst.addr, &new_value);
                        }
                    }
                }
            }
        }
    }

    Ok(SkBuffAction::Ignore)
}
