mod building_render;
mod mesh_render;
mod mesh_texture;
mod projection;
mod pyramid;
mod render;
mod server;
mod texture;
mod world;

use std::{io, num::NonZeroUsize, path::Path, sync::Arc};

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::{
    mesh_texture::MeshTextureSource,
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
    Prebuild {
        #[arg(long, default_value_t = default_jobs())]
        jobs: NonZeroUsize,
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
        Command::Prebuild { jobs } => {
            let aerial = AerialSource::open("data/aerial")?;
            let mesh_textures = MeshTextureSource::open(
                "data/clean/mesh-textures",
                &world.texture_ids,
                world.texture_sha256,
            )?;
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(jobs.get())
                .build()
                .map_err(io::Error::other)?;
            println!("prebuild using {jobs} workers");
            pool.install(|| prebuild(&world, &aerial, &mesh_textures))
        }
        Command::Serve { port } => serve(world, port).await,
    }
}

fn default_jobs() -> NonZeroUsize {
    std::thread::available_parallelism()
        .ok()
        .and_then(|jobs| NonZeroUsize::new(jobs.get().saturating_mul(4).min(32)))
        .unwrap_or(NonZeroUsize::MIN)
}
