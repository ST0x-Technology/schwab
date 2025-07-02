use clap::Parser;

#[derive(Parser, Debug)]
pub struct Env {
    #[clap(short, long, env)]
    pub ws_rpc_url: String,
}
