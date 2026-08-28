mod building_render;
mod mesh_render;
mod pyramid;
mod render;
mod server;
mod texture;
mod world;

use std::{io, path::Path, sync::Arc};

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::{
    server::{prebuild, serve},
    texture::AerialSource,
    world::load_world,
};

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
    Prebuild,
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
    let aerial = Arc::new(AerialSource::open("data/aerial")?);
    match cli.command {
        Command::Prebuild => prebuild(&world, &aerial),
        Command::Serve { port } => serve(world, aerial, port).await,
    }
}
