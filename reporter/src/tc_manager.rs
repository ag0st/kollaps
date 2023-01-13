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

use libloading::{Library, Symbol};

/// Interacts with TC library
pub struct TCManager {
    library: Library,

}

impl TCManager {
    pub fn new() -> TCManager {
        unsafe {
            TCManager {
                library: Library::new("/usr/local/bin/libTCAL.so").unwrap(),
            }
        }
    }

    pub fn init(&mut self, ip: u32) {
        unsafe {
            let udp_port = 7073;
            let tc_init: Symbol<unsafe extern fn(u32, u32, u32)> = self.library.get(b"init").unwrap();
            tc_init(udp_port, 1000, ip);
        }
    }

    pub fn initialize_path(&mut self, ip: u32, bandwidth: u32, latency: f32, jitter: f32, drop: f32) {
        unsafe {
            let init_destination: Symbol<unsafe extern fn(u32, u32, f32, f32, f32)> = self.library.get(b"initDestination").unwrap();

            init_destination(ip, bandwidth, latency, jitter, drop);
        }
    }

    pub fn disable_path(&mut self, ip: u32) {
        unsafe {
            let init_destination: Symbol<unsafe extern fn(u32, u32, f32, f32, f32)> = self.library.get(b"initDestination").unwrap();
            init_destination(ip, 10000, 1.0, 0.0, 1.0);
        }
    }

    pub fn change_bandwidth(&mut self, ip: u32, bandwidth: u32) {
        unsafe {
            let change_bw: Symbol<unsafe extern fn(u32, u32)> = self.library.get(b"changeBandwidth").unwrap();
            change_bw(ip, bandwidth / 1000);
        }
    }

    pub fn change_loss(&mut self, ip: u32, loss: f32) {
        unsafe {
            let change_loss: Symbol<unsafe extern fn(u32, f32)> = self.library.get(b"changeLoss").unwrap();
            change_loss(ip, loss);
        }
    }

    pub fn change_latency(&mut self, ip: u32, latency: f32, jitter: f32) {
        unsafe {
            let change_latency: Symbol<unsafe extern fn(u32, f32, f32)> = self.library.get(b"changeLatency").unwrap();
            change_latency(ip, latency, jitter);
        }
    }

    pub fn disconnect(&mut self) {
        unsafe {
            let disconnect: Symbol<unsafe extern fn()> = self.library.get(b"disconnect").unwrap();
            disconnect();
        }
    }


    pub fn reconnect(&mut self) {
        unsafe {
            let reconnect: Symbol<unsafe extern fn()> = self.library.get(b"reconnect").unwrap();
            reconnect();
        }
    }

    pub fn tear_down(&mut self) {
        unsafe {
            let teardown: Symbol<unsafe extern fn(u32)> = self.library.get(b"tearDown").unwrap();
            teardown(0);
        }
    }
}