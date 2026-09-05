use crate::world::View;

const PYRAMID_VERSION: &str = "v57-inferred-roofs";
// The multi-angle mesh viewer is hidden from the launch UI. Its v54 atlas
// already has the baked continuous-color finish. The later canopy and
// metre-correct morphology changes are intentionally citywide-only because the
// rich mesh already photographs local vegetation and building form. Keep that
// verified atlas immutable.
const BAKED_RICH_IDENTITY: &str = "v54-shared-palette-rich-style";

pub(crate) fn base_tile_version(
    world_sha256: &[u8; 32],
    land_cover_sha256: Option<&[u8; 32]>,
) -> String {
    base_tile_version_hex(
        &digest_hex(world_sha256),
        land_cover_sha256.map(digest_hex).as_deref(),
    )
}

pub(crate) fn base_tile_version_hex(world_sha256: &str, land_cover_sha256: Option<&str>) -> String {
    let world_prefix = &world_sha256[..16];
    match land_cover_sha256 {
        Some(digest) => format!("{PYRAMID_VERSION}-{world_prefix}-lc-{digest}"),
        None => format!("{PYRAMID_VERSION}-{world_prefix}"),
    }
}

pub(crate) fn rich_tile_version(tile_version: &str, view: View) -> String {
    let rich_source = if tile_version.starts_with(PYRAMID_VERSION) {
        BAKED_RICH_IDENTITY.to_owned()
    } else {
        tile_version.to_owned()
    };
    format!(
        "{rich_source}-rich-{}-z{}-full",
        view.id(),
        crate::pyramid::RICH_ART_ZOOM
    )
}

pub(crate) fn is_generation_of(candidate: &str, base: &str) -> bool {
    candidate == base
        || candidate
            .strip_prefix(base)
            .and_then(|suffix| suffix.strip_prefix("-r"))
            .is_some_and(|revision| {
                revision
                    .as_bytes()
                    .first()
                    .is_some_and(|byte| (b'1'..=b'9').contains(byte))
                    && revision.bytes().skip(1).all(|byte| byte.is_ascii_digit())
            })
}

fn digest_hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::{BAKED_RICH_IDENTITY, base_tile_version, is_generation_of, rich_tile_version};
    use crate::world::View;

    #[test]
    fn identity_binds_optional_land_cover_artifact() {
        let world = [0x11; 32];
        let first = [0x22; 32];
        let second = [0x33; 32];

        assert_eq!(
            base_tile_version(&world, None),
            "v57-inferred-roofs-1111111111111111"
        );
        assert_ne!(
            base_tile_version(&world, Some(&first)),
            base_tile_version(&world, Some(&second))
        );
        assert!(base_tile_version(&world, Some(&first)).ends_with(&"22".repeat(32)));
    }

    #[test]
    fn revisions_and_rich_views_have_exact_names() {
        assert!(is_generation_of("v1-abc", "v1-abc"));
        assert!(is_generation_of("v1-abc-r2", "v1-abc"));
        assert!(!is_generation_of("v1-abc-r", "v1-abc"));
        assert!(!is_generation_of("v1-abc-r0", "v1-abc"));
        assert!(!is_generation_of("v1-abc-r02", "v1-abc"));
        assert!(!is_generation_of("v1-abcd", "v1-abc"));
        assert_eq!(
            rich_tile_version("v1-abc", View::NorthWest),
            "v1-abc-rich-nw-z5-full"
        );
        let citywide = base_tile_version(&[0x11; 32], Some(&[0x22; 32]));
        assert_eq!(
            rich_tile_version(&citywide, View::SouthEast),
            format!("{BAKED_RICH_IDENTITY}-rich-se-z5-full")
        );
    }
}
