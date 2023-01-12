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

use ::core::fmt;
use ::core::mem::transmute;

#[derive(Copy, Clone)]
#[repr(C)]
pub struct Message {
    pub dst: u32,
    pub throughput: u32,
}

#[repr(C)]
pub struct SocketAddr {
    pub addr: u32,
}

impl fmt::Display for SocketAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let octets: [u8; 4] = unsafe { transmute::<u32, [u8; 4]>(self.addr) };

        write!(
            f,
            "{:^3}.{:^3}.{:^3}.{:^3}",
            octets[3], octets[2], octets[1], octets[0]
        )
    }
}

impl SocketAddr {
    pub fn new(addr: u32) -> Self {
        SocketAddr {
            addr,
        }
    }
}

