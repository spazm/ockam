use std::collections::HashMap;

use anyhow::{anyhow, Context as _};
use clap::Args;
use ockam_api::config::lookup::InternetAddress;
use ockam_multiaddr::proto::{DnsAddr, Ip4, Ip6, Project, Tcp};
use rand::prelude::random;

use ockam::{Context, TcpTransport};
use ockam_api::is_local_node;
use ockam_api::nodes::models::forwarder::{CreateForwarder, ForwarderInfo};
use ockam_api::nodes::models::secure_channel::CredentialExchangeMode;
use ockam_core::api::Request;
use ockam_multiaddr::{proto::Node, MultiAddr, Protocol};

use crate::forwarder::HELP_DETAIL;
use crate::project::util;
use crate::util::api::CloudOpts;
use crate::util::output::Output;
use crate::util::{get_final_element, node_rpc, RpcBuilder};
use crate::Result;
use crate::{help, CommandGlobalOpts};

/// Create Forwarders
#[derive(Clone, Debug, Args)]
#[command(
    arg_required_else_help = true,
    help_template = help::template(HELP_DETAIL)
)]
pub struct CreateCommand {
    /// Name of the forwarder (optional)
    #[arg(hide_default_value = true, default_value_t = hex::encode(&random::<[u8;4]>()))]
    forwarder_name: String,

    /// Node for which to create the forwarder
    #[arg(long, id = "NODE", display_order = 900)]
    to: String,

    /// Route to the node at which to create the forwarder (optional)
    #[arg(long, id = "ROUTE", display_order = 900)]
    at: MultiAddr,

    /// Orchestrator address to resolve projects present in the `at` argument
    #[command(flatten)]
    cloud_opts: CloudOpts,
}

impl CreateCommand {
    pub fn run(self, options: CommandGlobalOpts) {
        node_rpc(rpc, (options, self));
    }
}

async fn rpc(ctx: Context, (opts, cmd): (CommandGlobalOpts, CreateCommand)) -> Result<()> {
    let tcp = TcpTransport::create(&ctx).await?;
    let api_node = get_final_element(&cmd.to);
    let at_rust_node = is_local_node(&cmd.at).context("Argument --at is not valid")?;

    let lookup = opts.config.lookup();

    let mut ma = MultiAddr::default();
    let mut pa = HashMap::new();

    for proto in cmd.at.iter() {
        match proto.code() {
            Node::CODE => {
                let alias = proto
                    .cast::<Node>()
                    .ok_or_else(|| anyhow!("invalid node address protocol"))?;
                let addr = lookup
                    .get_node(&alias)
                    .ok_or_else(|| anyhow!("unknown node {}", &*alias))?;
                match addr {
                    InternetAddress::Dns(dns, _) => ma.push_back(DnsAddr::new(dns))?,
                    InternetAddress::V4(v4) => ma.push_back(Ip4(*v4.ip()))?,
                    InternetAddress::V6(v6) => ma.push_back(Ip6(*v6.ip()))?,
                }
                ma.push_back(Tcp(addr.port()))?
            }
            Project::CODE => {
                let alias = proto
                    .cast::<Project>()
                    .ok_or_else(|| anyhow!("invalid project address protocol"))?;
                if lookup.get_project(&alias).is_none() {
                    util::config::refresh_projects(
                        &ctx,
                        &opts,
                        api_node,
                        &cmd.cloud_opts.route(),
                        Some(&tcp),
                    )
                    .await?
                }
                if let Some(p) = lookup.get_project(&alias) {
                    ma.try_extend(&p.node_route)?;
                    pa.insert(p.node_route.clone(), p.identity_id.clone());
                } else {
                    return Err(anyhow!("unknown project name {}", &*alias).into());
                }
            }
            _ => ma.push_back_value(&proto)?,
        }
    }

    let req = {
        let alias = if at_rust_node {
            format!("forward_to_{}", cmd.forwarder_name)
        } else {
            cmd.forwarder_name.clone()
        };
        let body = CreateForwarder::new(
            ma,
            Some(alias),
            at_rust_node,
            pa,
            CredentialExchangeMode::Oneway,
        );
        Request::post("/node/forwarder").body(body)
    };

    let mut rpc = RpcBuilder::new(&ctx, &opts, api_node).tcp(&tcp)?.build();
    rpc.request(req).await?;
    rpc.parse_and_print_response::<ForwarderInfo>()?;

    Ok(())
}

impl Output for ForwarderInfo<'_> {
    fn output(&self) -> anyhow::Result<String> {
        Ok(format!("/service/{}", self.remote_address()))
    }
}
