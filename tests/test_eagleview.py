from __future__ import annotations

import json
import unittest

import httpx
from pydantic import ValidationError

from geo_philly_ingest.eagleview import (
    ORTHOMOSAIC_SEARCH_PATH,
    RANK_LOCATION_PATH,
    TOKEN_URL,
    Bounds,
    EagleViewAccess,
    EagleViewClient,
    EagleViewSettings,
    build_plan,
)


class EagleViewTests(unittest.TestCase):
    def test_settings_parse_documented_environment_names_and_redact_secret(self) -> None:
        settings = EagleViewSettings.model_validate(
            {
                "EAGLE_VIEW_CLIENT_ID": "test-id",
                "EAGLE_VIEW_CLIENT_SECRET": "test-secret",
                "GEO_PHILLY_EAGLEVIEW_ENVIRONMENT": "production",
            }
        )

        self.assertEqual(settings.access(), EagleViewAccess("production", "test-id", "test-secret"))
        self.assertNotIn("test-secret", repr(settings))

    def test_settings_require_credentials(self) -> None:
        with self.assertRaises(ValidationError):
            EagleViewSettings(_env_file=None)

    def test_plan_grid_and_manifest_are_deterministic(self) -> None:
        plan = build_plan(Bounds(0, 0, 1_100, 600), cell_size_m=500)

        self.assertEqual((plan.rows, plan.columns), (2, 3))
        self.assertEqual(
            [cell.id for cell in plan.cells],
            [
                "r000-c000",
                "r000-c001",
                "r000-c002",
                "r001-c000",
                "r001-c001",
                "r001-c002",
            ],
        )
        self.assertEqual(plan.cells[-1].bounds, Bounds(1_000, 500, 1_100, 600))
        encoded = json.dumps(plan.manifest(), sort_keys=True)
        self.assertEqual(encoded, json.dumps(plan.manifest(), sort_keys=True))
        self.assertIn(ORTHOMOSAIC_SEARCH_PATH, encoded)
        self.assertIn(RANK_LOCATION_PATH, encoded)
        self.assertNotIn("client_secret", encoded)

    def test_mocked_client_authenticates_once_and_parses_discovery(self) -> None:
        requests: list[httpx.Request] = []

        def handler(request: httpx.Request) -> httpx.Response:
            requests.append(request)
            if str(request.url) == TOKEN_URL:
                self.assertTrue(request.headers["authorization"].startswith("Basic "))
                self.assertEqual(request.content, b"grant_type=client_credentials")
                return httpx.Response(
                    200,
                    json={"access_token": "token", "token_type": "Bearer", "expires_in": 3600},
                )
            self.assertEqual(request.headers["authorization"], "Bearer token")
            if request.url.path == ORTHOMOSAIC_SEARCH_PATH:
                return httpx.Response(
                    200,
                    json={
                        "orthomosaics": [
                            {
                                "urn": "urn:eagleview:test:ortho",
                                "category": "ORTHOMOSAIC_CATEGORY_VISUAL",
                                "level": "ORTHOMOSAIC_LEVEL_MEASURED",
                            }
                        ]
                    },
                )
            if request.url.path == RANK_LOCATION_PATH:
                return httpx.Response(
                    200,
                    json={
                        "captures": [
                            {
                                "capture": {"start_date": "2025-01-01", "end_date": "2025-01-02"},
                                "orthos": {"images": [{"urn": "urn:eagleview:test:top"}]},
                                "obliques": {
                                    "north": {"images": [{"urn": "urn:eagleview:test:north"}]}
                                },
                            }
                        ]
                    },
                )
            return httpx.Response(404)

        access = EagleViewAccess("sandbox", "test-id", "test-secret")
        plan = build_plan(Bounds(0, 0, 500, 500))
        with EagleViewClient(access, transport=httpx.MockTransport(handler)) as client:
            orthomosaics = client.search_orthomosaics(plan.aoi)
            images = client.rank(plan.cells[0])

        self.assertEqual(orthomosaics[0].urn, "urn:eagleview:test:ortho")
        self.assertEqual([image.view for image in images], ["ortho", "north"])
        self.assertEqual(sum(str(request.url) == TOKEN_URL for request in requests), 1)


if __name__ == "__main__":
    unittest.main()
