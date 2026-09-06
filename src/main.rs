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
mod terrain;
mod texture;
mod tile_codec;
mod tile_identity;
mod transport_render;
mod tree_render;
mod world;

use std::{io, path::Path};

use clap::{Parser, Subcommand};
use serde::Serialize;
use tracing_subscriber::EnvFilter;

use crate::{
    building_render::eligible_detached_frontage,
    land_cover::LandCoverMask,
    mesh_texture::MeshTextureSource,
    server::{prebuild, prebuild_is_complete, serve},
    terrain::Terrain,
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
    /// Print deterministic building-context and frontage coverage counts.
    WorldAudit,
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
            let terrain = Terrain::open_optional(Path::new("data/clean/terrain-v1.isoterrain"))?;
            let terrain_sha256 = terrain.as_ref().map(Terrain::artifact_sha256);
            if prebuild_is_complete(
                &world_digest(world_path)?,
                land_cover_sha256.as_ref(),
                terrain_sha256.as_ref(),
            ) {
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
            pool.install(|| {
                prebuild(
                    &world,
                    &aerial,
                    &mesh_textures,
                    land_cover.as_ref(),
                    terrain.as_ref(),
                )
            })
        }
        Command::Serve { port } => serve(port).await,
        Command::WorldAudit => {
            let world = load_world(Path::new("data/clean/philly.bin"))?;
            let audit = WorldAudit::from_world(&world)?;
            println!(
                "{}",
                serde_json::to_string(&audit).map_err(io::Error::other)?
            );
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
struct BuildingContextCounts {
    rowhouses: usize,
    rowhouse_like: usize,
    twins: usize,
    detached: usize,
    warehouses: usize,
    generic: usize,
}

#[derive(Serialize)]
struct WorldAudit {
    schema_version: u8,
    building_count: usize,
    building_kind_counts: BuildingContextCounts,
    known_frontage_counts_by_kind: BuildingContextCounts,
    eligible_detached_frontage_count: usize,
}

impl BuildingContextCounts {
    fn from_world(world: &World) -> Self {
        Self::from_contexts(world.building_contexts.iter())
    }

    fn from_contexts<'a>(
        contexts: impl Iterator<Item = &'a crate::world::BuildingContext>,
    ) -> Self {
        let mut counts = Self {
            rowhouses: 0,
            rowhouse_like: 0,
            twins: 0,
            detached: 0,
            warehouses: 0,
            generic: 0,
        };
        for context in contexts {
            match context.kind {
                BuildingKind::Rowhouse => counts.rowhouses += 1,
                BuildingKind::RowhouseLike => counts.rowhouse_like += 1,
                BuildingKind::Twin => counts.twins += 1,
                BuildingKind::Detached => counts.detached += 1,
                BuildingKind::Warehouse => counts.warehouses += 1,
                BuildingKind::Generic => counts.generic += 1,
            }
        }
        counts
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

impl WorldAudit {
    fn from_world(world: &World) -> io::Result<Self> {
        let building_kind_counts = BuildingContextCounts::from_world(world);
        let known_frontage_counts_by_kind = BuildingContextCounts::from_contexts(
            world
                .buildings
                .iter()
                .zip(&world.building_contexts)
                .filter_map(|(building, context)| {
                    building.frontage_edge.is_some().then_some(context)
                }),
        );
        let eligible_detached_frontage_count = world
            .buildings
            .iter()
            .zip(&world.building_contexts)
            .filter(|(building, context)| eligible_detached_frontage(building, context))
            .count();
        let audit = Self {
            schema_version: 2,
            building_count: world.buildings.len(),
            building_kind_counts,
            known_frontage_counts_by_kind,
            eligible_detached_frontage_count,
        };
        audit.validate()?;
        Ok(audit)
    }

    fn validate(&self) -> io::Result<()> {
        self.building_kind_counts.validate(self.building_count)?;
        let known = self.known_frontage_counts_by_kind;
        let total = self.building_kind_counts;
        if known.rowhouses > total.rowhouses
            || known.rowhouse_like > total.rowhouse_like
            || known.twins > total.twins
            || known.detached > total.detached
            || known.warehouses > total.warehouses
            || known.generic > total.generic
        {
            return Err(io::Error::other(
                "known frontage counts exceed their building-context counts",
            ));
        }
        if self.eligible_detached_frontage_count > known.detached {
            return Err(io::Error::other(
                "eligible detached frontage count exceeds known detached frontages",
            ));
        }
        Ok(())
    }
}

fn validate_building_contexts(world: &World) -> io::Result<()> {
    let counts = BuildingContextCounts::from_world(world);
    counts.validate(world.buildings.len())?;
    let named_frontages = world
        .buildings
        .iter()
        .filter(|building| building.frontage_edge.is_some())
        .count();
    let named_rowhouse_frontages = world
        .buildings
        .iter()
        .zip(&world.building_contexts)
        .filter(|(building, context)| {
            building.frontage_edge.is_some() && context.kind == BuildingKind::Rowhouse
        })
        .count();
    println!(
        "building contexts: {} attached rowhouses, {} rowhouse-like, {} twins, {} detached, {} warehouses, {} generic",
        counts.rowhouses,
        counts.rowhouse_like,
        counts.twins,
        counts.detached,
        counts.warehouses,
        counts.generic,
    );
    println!(
        "named frontages: {named_rowhouse_frontages} attached rowhouses from {named_frontages} packed candidates"
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
    use super::{BuildingContextCounts, WorldAudit};
    use crate::world::{BuildingContext, BuildingKind};

    fn context(kind: BuildingKind) -> BuildingContext {
        BuildingContext {
            kind,
            material_group_seed: 0,
            party_edge_mask: 0,
        }
    }

    #[test]
    fn context_count_aggregation_keeps_each_kind_separate() {
        let contexts = [
            context(BuildingKind::Rowhouse),
            context(BuildingKind::Rowhouse),
            context(BuildingKind::RowhouseLike),
            context(BuildingKind::Twin),
            context(BuildingKind::Detached),
            context(BuildingKind::Warehouse),
            context(BuildingKind::Generic),
        ];

        assert_eq!(
            BuildingContextCounts::from_contexts(contexts.iter()),
            BuildingContextCounts {
                rowhouses: 2,
                rowhouse_like: 1,
                twins: 1,
                detached: 1,
                warehouses: 1,
                generic: 1,
            }
        );
    }

    #[test]
    fn world_audit_json_schema_is_stable() -> Result<(), Box<dyn std::error::Error>> {
        let audit = WorldAudit {
            schema_version: 2,
            building_count: 3,
            building_kind_counts: BuildingContextCounts {
                rowhouses: 1,
                rowhouse_like: 0,
                twins: 1,
                detached: 1,
                warehouses: 0,
                generic: 0,
            },
            known_frontage_counts_by_kind: BuildingContextCounts {
                rowhouses: 1,
                rowhouse_like: 0,
                twins: 0,
                detached: 1,
                warehouses: 0,
                generic: 0,
            },
            eligible_detached_frontage_count: 1,
        };

        audit.validate()?;

        assert_eq!(
            serde_json::to_string(&audit)?,
            r#"{"schema_version":2,"building_count":3,"building_kind_counts":{"rowhouses":1,"rowhouse_like":0,"twins":1,"detached":1,"warehouses":0,"generic":0},"known_frontage_counts_by_kind":{"rowhouses":1,"rowhouse_like":0,"twins":0,"detached":1,"warehouses":0,"generic":0},"eligible_detached_frontage_count":1}"#
        );
        Ok(())
    }

    #[test]
    fn world_audit_rejects_inconsistent_frontage_coverage() {
        let audit = WorldAudit {
            schema_version: 2,
            building_count: 1,
            building_kind_counts: BuildingContextCounts {
                rowhouses: 0,
                rowhouse_like: 0,
                twins: 0,
                detached: 1,
                warehouses: 0,
                generic: 0,
            },
            known_frontage_counts_by_kind: BuildingContextCounts {
                rowhouses: 0,
                rowhouse_like: 0,
                twins: 0,
                detached: 0,
                warehouses: 1,
                generic: 0,
            },
            eligible_detached_frontage_count: 1,
        };

        assert!(
            audit
                .validate()
                .is_err_and(|error| error.to_string().contains("known frontage counts"))
        );

        let audit = WorldAudit {
            known_frontage_counts_by_kind: BuildingContextCounts {
                rowhouses: 0,
                rowhouse_like: 0,
                twins: 0,
                detached: 1,
                warehouses: 0,
                generic: 0,
            },
            eligible_detached_frontage_count: 2,
            ..audit
        };
        assert!(audit.validate().is_err_and(|error| {
            error
                .to_string()
                .contains("eligible detached frontage count")
        }));
    }

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
