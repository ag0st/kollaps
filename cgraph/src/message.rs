use common::ClusterNodeInfo;
use crate::CGraph;

#[derive()]
pub enum CGraphUpdate {
    New(CGraph<ClusterNodeInfo>),
    Remove(ClusterNodeInfo),
}