mod render;
mod server;
mod world;

use std::{io, path::Path, sync::Arc};

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::{
    server::{prebuild, serve},
    world::load_world,
};

const PREBUILD_ZOOM: u8 = 5;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve {
        #[arg(long, default_value_t = 3000)]
        port: u16,
    },
    Prebuild {
        #[arg(long, default_value_t = PREBUILD_ZOOM)]
        max_zoom: u8,
    },
}

#[tokio::main]
async fn main() -> io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("geo_philly=info,tower_http=info")),
        )
        .compact()
        .init();
    let cli = Cli::parse();
    let world = Arc::new(load_world(Path::new("data/clean/philly.bin"))?);
    match cli.command {
        Command::Prebuild { max_zoom } => prebuild(&world, max_zoom),
        Command::Serve { port } => serve(world, port).await,
    }
}
