mod building_render;
mod mesh_render;
mod mesh_texture;
mod projection;
mod pyramid;
mod render;
mod scene;
mod server;
mod texture;
mod tile_codec;
mod world;

use std::{io, path::Path};

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::{
    mesh_texture::MeshTextureSource,
    server::{prebuild, prebuild_is_complete, serve},
    texture::AerialSource,
    world::{load_world, world_digest},
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
    Prebuild {
        #[arg(
            long,
            default_value_t = default_jobs(),
            value_parser = clap::value_parser!(u8).range(1..=16)
        )]
        jobs: u8,
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
    match cli.command {
        Command::Prebuild { jobs } => {
            let world_path = Path::new("data/clean/philly.bin");
            if prebuild_is_complete(&world_digest(world_path)?) {
                return Ok(());
            }
            let world = load_world(world_path)?;
            let aerial = AerialSource::open("data/aerial")?;
            let mesh_textures = MeshTextureSource::open(
                "data/clean/mesh-textures",
                &world.texture_ids,
                world.texture_sha256,
            )?;
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(usize::from(jobs))
                .build()
                .map_err(io::Error::other)?;
            println!("prebuild using {jobs} workers");
            pool.install(|| prebuild(&world, &aerial, &mesh_textures))
        }
        Command::Serve { port } => serve(port).await,
    }
}

fn default_jobs() -> u8 {
    std::thread::available_parallelism()
        .ok()
        .map(|jobs| jobs.get().min(16) as u8)
        .unwrap_or(1)
}
