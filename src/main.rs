mod building_render;
mod land_cover;
mod mesh_render;
mod mesh_texture;
mod palette;
mod projection;
mod pyramid;
mod render;
mod scene;
mod server;
mod shadow_render;
mod texture;
mod tile_codec;
mod tile_identity;
mod transport_render;
mod tree_render;
mod world;

use std::{io, path::Path};

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::{
    land_cover::LandCoverMask,
    mesh_texture::MeshTextureSource,
    server::{prebuild, prebuild_is_complete, serve},
    texture::AerialSource,
    world::{BuildingKind, World, load_world, world_digest},
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
                .unwrap_or_else(|_| EnvFilter::new("isophilly=info,tower_http=info")),
        )
        .compact()
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Prebuild { jobs } => {
            let world_path = Path::new("data/clean/philly.bin");
            let land_cover =
                LandCoverMask::open_optional(Path::new("data/clean/land-cover-2018.isomask"))?;
            let land_cover_sha256 = land_cover.as_ref().map(LandCoverMask::artifact_sha256);
            if prebuild_is_complete(&world_digest(world_path)?, land_cover_sha256.as_ref()) {
                return Ok(());
            }
            let world = load_world(world_path)?;
            validate_building_contexts(&world)?;
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
            pool.install(|| prebuild(&world, &aerial, &mesh_textures, land_cover.as_ref()))
        }
        Command::Serve { port } => serve(port).await,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BuildingContextCounts {
    rowhouses: usize,
    rowhouse_like: usize,
    twins: usize,
    detached: usize,
    warehouses: usize,
    generic: usize,
}

impl BuildingContextCounts {
    fn from_world(world: &World) -> Self {
        let count = |kind| {
            world
                .building_contexts
                .iter()
                .filter(|context| context.kind == kind)
                .count()
        };
        Self {
            rowhouses: count(BuildingKind::Rowhouse),
            rowhouse_like: count(BuildingKind::RowhouseLike),
            twins: count(BuildingKind::Twin),
            detached: count(BuildingKind::Detached),
            warehouses: count(BuildingKind::Warehouse),
            generic: count(BuildingKind::Generic),
        }
    }

    fn total(self) -> usize {
        self.rowhouses
            + self.rowhouse_like
            + self.twins
            + self.detached
            + self.warehouses
            + self.generic
    }

    fn inferred_residential(self) -> usize {
        self.rowhouses + self.rowhouse_like + self.twins
    }

    fn validate(self, building_count: usize) -> io::Result<()> {
        if self.total() != building_count {
            return Err(io::Error::other(format!(
                "building context count {} does not match {building_count} buildings",
                self.total()
            )));
        }

        // These intentionally broad bounds are not a claim about land use. They
        // are a production tripwire for interpreting EPSG:32129 horizontal metres
        // as feet: that regression classified most of Philadelphia as large
        // warehouses and almost no narrow residential footprints.
        if building_count >= 100_000
            && (self.warehouses > building_count / 4
                || self.inferred_residential() < building_count / 400)
        {
            return Err(io::Error::other(format!(
                "implausible building context distribution: {} warehouses and {} inferred narrow residential footprints out of {building_count}; check horizontal units",
                self.warehouses,
                self.inferred_residential()
            )));
        }
        Ok(())
    }
}

fn validate_building_contexts(world: &World) -> io::Result<()> {
    let counts = BuildingContextCounts::from_world(world);
    counts.validate(world.buildings.len())?;
    println!(
        "building contexts: {} attached rowhouses, {} rowhouse-like, {} twins, {} detached, {} warehouses, {} generic",
        counts.rowhouses,
        counts.rowhouse_like,
        counts.twins,
        counts.detached,
        counts.warehouses,
        counts.generic,
    );
    Ok(())
}

fn default_jobs() -> u8 {
    std::thread::available_parallelism()
        .ok()
        .map(|jobs| jobs.get().min(16) as u8)
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::BuildingContextCounts;

    #[test]
    fn building_context_sanity_guard_accepts_a_broad_plausible_distribution() {
        let counts = BuildingContextCounts {
            rowhouses: 5_000,
            rowhouse_like: 30_000,
            twins: 1_000,
            detached: 75_000,
            warehouses: 20_000,
            generic: 369_000,
        };

        assert!(counts.validate(500_000).is_ok());
    }

    #[test]
    fn building_context_sanity_guard_rejects_horizontal_unit_regression() {
        let counts = BuildingContextCounts {
            rowhouses: 40,
            rowhouse_like: 1_000,
            twins: 200,
            detached: 12_000,
            warehouses: 386_760,
            generic: 100_000,
        };

        assert!(
            counts
                .validate(500_000)
                .is_err_and(|error| error.to_string().contains("check horizontal units"))
        );
    }

    #[test]
    fn building_context_sanity_guard_checks_count_completeness() {
        let counts = BuildingContextCounts {
            rowhouses: 1,
            rowhouse_like: 1,
            twins: 1,
            detached: 1,
            warehouses: 1,
            generic: 1,
        };

        assert!(
            counts
                .validate(7)
                .is_err_and(|error| error.to_string().contains("does not match"))
        );
    }
}
