mod building_render;
mod mesh_render;
mod render;
mod server;
mod texture;
mod world;

use std::{io, path::Path, sync::Arc};

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::{
    server::{prebuild, serve},
    texture::{AerialSource, TextureMode},
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
        #[arg(long, value_enum, default_value_t)]
        texture: TextureMode,
    },
    Prebuild {
        #[arg(long, default_value_t = PREBUILD_ZOOM)]
        max_zoom: u8,
        #[arg(long, value_enum, default_value_t)]
        texture: TextureMode,
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
        Command::Prebuild { max_zoom, texture } => {
            let aerial = aerial_source(texture)?;
            prebuild(&world, aerial.as_deref(), texture, max_zoom)
        }
        Command::Serve { port, texture } => {
            let aerial = aerial_source(texture)?;
            serve(world, aerial, texture, port).await
        }
    }
}

fn aerial_source(texture: TextureMode) -> io::Result<Option<Arc<AerialSource>>> {
    match texture {
        TextureMode::None => Ok(None),
        TextureMode::Full | TextureMode::Pixel => {
            AerialSource::open("data/aerial").map(Arc::new).map(Some)
        }
    }
}
