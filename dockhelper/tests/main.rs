#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::str::FromStr;

    use common::Subnet;
    use dockhelper::DockerHelper;

    #[tokio::test]
    async fn main() {
        let gateway = "192.168.1.1";
        let subnet = Subnet::from(("192.168.1.0", 24));
        let interface = "wlp0s20f3";

        let helper = DockerHelper::init(interface, subnet, gateway).await.unwrap();
        let res = helper.launch_container("alpine:latest", "ipvlan-1", IpAddr::from_str("192.168.1.200").unwrap(), Some("ash")).await.unwrap();
        println!("{}", res);
        let res = helper.launch_container("alpine:latest", "ipvlan-2", IpAddr::from_str("192.168.1.201").unwrap(), Some("ash")).await.unwrap();
        println!("{}", res);


        let res = helper.stop_container("ipvlan-1").await.unwrap();
        println!("{}", res);

        let res = helper.stop_container("ipvlan-2").await.unwrap();
        println!("{}", res);

        helper.delete_network().await.unwrap();
    }
}
