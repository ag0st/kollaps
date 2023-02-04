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

use std::collections::HashMap;

use roxmltree::{Document, Node};
use regex::Regex;
use std::net::IpAddr;
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;
use common::{Result, Error, ErrorKind, EmulationEvent, EventAction};
use netgraph::{Network, PathStrategy};
use crate::data::{Application, ApplicationKind, ContainerConfig};

fn produce_err(text: &str) -> Result<()> {
    Err(Error::new("parse topology", ErrorKind::Parse, text))
}

pub fn parse_topology(document_text: String, emul_id: Uuid) -> Result<(Network<crate::data::Node>, Vec<EmulationEvent>)> {
    let doc = Document::parse(&document_text).unwrap();
    let root = doc.root().first_child().unwrap();

    if root.tag_name().name() != "experiment" {
        produce_err("Not a valid Kollaps topology file, root is not <experiment>")?;
    }

    if !root.has_attribute("boot") {
        produce_err("<experiment boot = > The experiment needs a valid boostrapper image name")?;
    }

    let mut services: Option<Node> = None;
    let mut bridges: Option<Node> = None;
    let mut links: Option<Node> = None;
    let mut dynamic: Option<Node> = None;

    for node in root.children() {
        if !node.is_element() {
            continue;
        }
        if node.tag_name().name() == "services" {
            if !services.is_none() {
                produce_err("Only one <services> block is allowed.")?;
            }
            services = Some(node);
        }
        if node.tag_name().name() == "bridges" {
            if !bridges.is_none() {
                produce_err("Only one <bridges> block is allowed.")?;
            }
            bridges = Some(node);
        }
        if node.tag_name().name() == "links" {
            if !links.is_none() {
                produce_err("Only one <links> block is allowed.")?;
            }
            links = Some(node);
        }
        if node.tag_name().name() == "dynamic" {
            if !dynamic.is_none() {
                produce_err("Only one <dynamic> block is allowed.")?;
            }
            dynamic = Some(node);
        }
    }

    // Hashmap that will contains all the future nodes in our graph
    let mut nodes_map = HashMap::new();

    if services.is_none() {
        produce_err("No services declared in topology")?;
    }
    parse_services(services.unwrap(), emul_id, &mut nodes_map)?;

    if bridges.is_some() {
        parse_bridges(bridges.unwrap(), &mut nodes_map)?;
    }

    // Now we can create the Network
    let vertices = nodes_map.values().map(|v| v.clone()).collect::<Vec<crate::data::Node>>();
    let mut net = Network::new(&vertices, PathStrategy::ShortestPath);

    if links.is_none() {
        produce_err("No links declared in topology")?;
    }
    parse_links(links.unwrap(), &nodes_map, &mut net)?;

    // Finally we can parse the schedule
    let schedule = parse_schedule(&nodes_map, dynamic)?;
    Ok((net, schedule))
}

fn produce_name_for_container(emul_id: Uuid, node_id: u32) -> String {
    format!("kollaps_{}_{}", emul_id, node_id)
}


pub fn parse_services(services: Node, emul_id: Uuid, nodes: &mut HashMap<String, crate::data::Node>) -> Result<()> {
    let mut current_node: u32 = nodes.len() as u32;

    for service in services.children() {
        if !service.is_element() {
            continue;
        }
        if !(service.tag_name().name() == "service") {
            produce_err(&*format!("Invalid tag inside <services> {}", service.tag_name().name()))?;
        }

        // Creating the Service Kind
        let mut service_kind = None;
        if service.has_attribute("kind") {
            // two case scenario: 1st only container without attributes
            //                    2nd image + command attributes
            if service.attribute("kind").is_none() {
                produce_err("You must specify the kind of the service, you cannot let it blank [container | baremetal]")?;
            } else {
                match &*service.attribute("kind").unwrap().to_uppercase() {
                    "CONTAINER" => {
                        // two case scenario: 1st only container without attributes + ip
                        //                    2nd image + command attributes
                        if !service.has_attribute("image") && !service.has_attribute("command") && service.has_attribute("name") && service.has_attribute("ip") {
                            // This is the first case scenario
                            produce_err("Attaching to an existing container is not supported for now")?
                        } else if service.has_attribute("image") && service.has_attribute("command") && service.attribute("image").is_some() {
                            let command = if service.attribute("command").is_some() {
                                Some(service.attribute("command").unwrap().to_string())
                            } else {
                                None
                            };
                            let kind = ApplicationKind::Container(
                                ContainerConfig::new(produce_name_for_container(emul_id, current_node),
                                                     None,
                                                     Some((service.attribute("image").unwrap().to_string(), command)),
                                ));
                            service_kind = Some(kind);
                        } else {
                            // Error if this happens
                            produce_err("A Container Service must have no image+command attribute if attaching to existing one, or have image+command if we must deploy one.")?
                        }
                    }
                    "BAREMETAL" => {
                        service_kind = Some(ApplicationKind::BareMetal)
                    }
                    &_ => produce_err("Kind can be either baremetal | container")?
                }
            }
        } else {
            produce_err("No \"kind\" attribute detected on the service!")?;
        }


        // We can get the name
        let name = if service.has_attribute("name") && service.attribute("name").is_some() {
            service.attribute("name").unwrap().to_string()
        } else {
            produce_err("A service must have a name")?;
            "".to_string()
        };

        let ip = if service.has_attribute("ip") && service.attribute("ip").is_some() {
            Some(IpAddr::from_str(service.attribute("ip").unwrap()).unwrap())
        } else {
            None
        };
        // Finally create the Service
        let serv = Application::build(ip, None, name.clone(), service_kind.unwrap());
        nodes.insert(name, crate::data::Node::App(current_node, serv));
        current_node = current_node + 1;
    }
    Ok(())
}

#[allow(dead_code)]
pub fn process_active_paths(activepaths: &str) -> Vec<String> {
    let activepaths = activepaths.replace("[", "");
    let activepaths = activepaths.replace("]", "");

    let activepaths = activepaths.split(",");

    let mut vector_paths = vec![];
    for path in activepaths {
        let path = path.replace("'", "");
        vector_paths.push(path.to_string());
    }
    return vector_paths;
}

pub fn parse_bridges(bridges: Node, nodes: &mut HashMap<String, crate::data::Node>) -> Result<()> {
    let mut current_node = nodes.len() as u32;
    for bridge in bridges.children() {
        if !bridge.is_element() {
            continue;
        }
        if bridge.tag_name().name() != "bridge" {
            produce_err(&*format!("Invalid tag inside <bridges>: {}", bridge.tag_name().name()))?;
        }

        if !bridge.has_attribute("name") {
            produce_err("A bridge needs to have a name")?;
        }
        nodes.insert(bridge.attribute("name").unwrap().to_string(), crate::data::Node::Bridge(current_node));
        current_node = current_node + 1;
    }
    Ok(())
}

pub fn parse_links(links: Node, name_to_node: &HashMap<String, crate::data::Node>, network: &mut Network<crate::data::Node>) -> Result<()> {
    for link in links.children() {
        if !link.is_element() {
            continue;
        }
        if link.tag_name().name() != "link" {
            produce_err(&*format!("Invalid tag inside <link> {}", link.tag_name().name()))?;
        }

        if !link.has_attribute("origin") || !link.has_attribute("dest") || !link.has_attribute("bandwidth") {
            produce_err("Incomplete network description")?;
        }

        let source = link.attribute("origin").unwrap().to_string();
        let source = name_to_node.get(&source).unwrap();
        let destination = link.attribute("dest").unwrap().to_string();
        let destination = name_to_node.get(&destination).unwrap();
        let bandwidth: String = link.attribute("bandwidth").unwrap().to_string();
        let bandwidth = parse_bandwidth(bandwidth);

        let mut jitter: f32 = 0.0;
        if link.has_attribute("jitter") {
            jitter = link.attribute("jitter").unwrap().parse().unwrap();
        }
        let mut latency: f32 = 0.0;
        if link.has_attribute("latency") {
            latency = link.attribute("latency").unwrap().parse().unwrap();
        }
        let mut drop: f32 = 0.0;
        if link.has_attribute("drop") {
            drop = link.attribute("drop").unwrap().parse().unwrap();
        }
        // Create the link inside the graph
        network.add_edge_with_props(source, destination, bandwidth? as u32, (latency, jitter), drop);
    }
    Ok(())
}

pub fn parse_bandwidth(bandwidth: String) -> Result<f32> {
    let bandwidth_regex = Regex::new(r"([0-9]+)([KMG])bps").unwrap();

    if bandwidth_regex.is_match(&bandwidth) {
        let captures = bandwidth_regex.captures(&bandwidth).unwrap();
        let mut base: f32 = captures.get(1).unwrap().as_str().parse().unwrap();
        let multiplier = captures.get(2).unwrap().as_str();
        if multiplier == "K" {
           // base = base * 1000.0;
        }
        if multiplier == "M" {
            base = base * 1024.0;
        }
        if multiplier == "G" {
            base = base * 1024.0 * 1024.0;
        }
        Ok(base)
    } else {
        //print and fail
        Err(Error::new("xml parse bandwidth", ErrorKind::Parse, "cannot parse the bandwidth"))
    }
}

pub fn parse_schedule(name_to_service: &HashMap<String, crate::data::Node>, dynamic: Option<Node>) -> Result<Vec<EmulationEvent>> {
    let mut result = Vec::new();
    // No dynamic events
    if dynamic.is_none() {
        return Ok(result);
    }
    let dynamic = dynamic.unwrap();

    for event in dynamic.children() {
        if !event.is_element() {
            continue;
        }

        if !(event.tag_name().name() == "schedule") {
            produce_err(&*format!("Only <schedule> is allowed inside <dynamic> {} ", event.tag_name().name().to_string()))?;
        }

        let mut time = 0.0;
        if event.has_attribute("time") {
            time = event.attribute("time").unwrap().parse().unwrap();

            if time < 0.0 {
                produce_err("time attribute must be a positive number")?;
            }
        }

        if !event.has_attribute("name") && !event.has_attribute("action") {
            produce_err("Event must have an associated name and action")?
        }

        let node_name = event.attribute("name").unwrap().to_string();
        let action = match &*event.attribute("action").unwrap().to_uppercase() {
            "JOIN" => EventAction::Join,
            "LEAVE" => EventAction::Quit,
            "CRASH" => EventAction::Crash,
            &_ => {
                produce_err("Event action not recognized")?;
                EventAction::Crash
            }
        };
        // Create the event
        if let None = name_to_service.get(&node_name) {
            produce_err(&*format!("The node {} found in the scheduled event is not in the services.", node_name))?
        }
        result.push(EmulationEvent {
            app_id: name_to_service.get(&node_name).unwrap().id(),
            time: Duration::from_secs(time as u64),
            action,
        });
    }
    Ok(result)
}
